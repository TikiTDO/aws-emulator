// SPDX-License-Identifier: CC0-1.0

//! WHICH trail a `TrailNotFoundException` names, over the operations that address a
//! trail by its `Name` / `TrailName` member.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the
//! error code it branches on, and the `Message` it reads.
//!
//! The rules under test, each held over a SET of identifiers rather than for one
//! request:
//!
//! * an operation that takes the member as a trail name to LOOK UP — `GetTrail`,
//!   `GetInsightSelectors`, `GetEventConfiguration`, `PutEventConfiguration` — names
//!   the identifier the request supplied, in the form it supplied it, and never an ARN
//!   the service expanded a bare name into;
//! * an operation that resolves the member to ONE Region's trail in order to drive or
//!   configure it — the trail lifecycle, the logging switches and the event selectors —
//!   names that trail's fully-qualified ARN, whichever form the caller wrote;
//! * either way the account slot is the calling account (emulator state, not request
//!   input), the status is HTTP 400 and the code is `TrailNotFoundException`;
//! * a refusal is the whole answer: nothing is minted, no logging flag moves, and the
//!   trail that IS stored answers the same calls.
//!
//! The rows differ along what the rules generalise over: the identifier's FORM (bare
//! name, trail ARN), its length (each bound the model documents for a trail name), the
//! punctuation and case it carries, the Region its ARN pins and the account id its ARN
//! carries — the last two being the segments an expansion of a bare name would get
//! wrong.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_CATEGORY_AGGREGATION_VALUES, INSIGHT_TYPE_VALUES, TEMPLATE_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    trail_arn, TRAIL_NAME_MAX_LEN, TRAIL_NAME_MIN_LEN,
};

use super::emulator::{Emulator, HOME_REGION};

/// The status CloudTrail answers a not-found trail with on this awsJson1_1 API, where
/// the model annotates `@httpError(404)` (`state::TRAIL_NOT_FOUND_STATUS`).
const NOT_FOUND_STATUS: u16 = 400;

/// The destination bucket every trail in this suite delivers into.
const BUCKET: &str = "echo-refusal-bucket";

/// An account id that is NOT this emulator's: an ARN carrying it addresses the same
/// trail name, and a refusal that re-rendered the identifier from the service's own
/// account would lose it.
const OTHER_ACCOUNT: &str = "111122223333";

/// One well-formed `AggregationConfigurations` member, so a `PutEventConfiguration`
/// case is refused for its identifier and nothing else. Both values come from the
/// model's own enums (`Template`, `EventCategoryAggregation`).
fn aggregations() -> Value {
    json!([{
        "Templates": [TEMPLATE_VALUES[0]],
        "EventCategory": EVENT_CATEGORY_AGGREGATION_VALUES[0],
    }])
}

/// The operations that take the trail identifier as a name to LOOK UP: their refusal
/// names that identifier as the request wrote it. `mutating` marks the one that writes,
/// which runs in the trail's home Region only.
///
/// (operation, mutating, request body naming `identifier`)
fn name_echoing_requests(identifier: &str) -> Vec<(&'static str, bool, Value)> {
    vec![
        ("GetTrail", false, json!({ "Name": identifier })),
        (
            "GetInsightSelectors",
            false,
            json!({ "TrailName": identifier }),
        ),
        (
            "GetEventConfiguration",
            false,
            json!({ "TrailName": identifier }),
        ),
        (
            "PutEventConfiguration",
            true,
            json!({ "TrailName": identifier, "AggregationConfigurations": aggregations() }),
        ),
    ]
}

/// The operations that resolve the identifier to ONE Region's trail in order to drive
/// or configure it: their refusal names that trail's ARN.
fn arn_naming_requests(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetTrailStatus", json!({ "Name": identifier })),
        (
            "UpdateTrail",
            json!({ "Name": identifier, "S3BucketName": BUCKET }),
        ),
        ("StartLogging", json!({ "Name": identifier })),
        ("StopLogging", json!({ "Name": identifier })),
        ("DeleteTrail", json!({ "Name": identifier })),
        ("GetEventSelectors", json!({ "TrailName": identifier })),
        (
            "PutEventSelectors",
            json!({
                "TrailName": identifier,
                "EventSelectors": [{ "IncludeManagementEvents": true, "ReadWriteType": "All" }],
            }),
        ),
        (
            "PutInsightSelectors",
            json!({
                "TrailName": identifier,
                "InsightSelectors": [{ "InsightType": INSIGHT_TYPE_VALUES[0] }],
            }),
        ),
    ]
}

/// (case, a bare name nothing carries) — the length bounds come from the rules the
/// model documents for `CreateTrailRequest$Name` ("Be between 3 and 128 characters"),
/// as `state::TRAIL_NAME_MIN_LEN` / `TRAIL_NAME_MAX_LEN` carry them.
fn absent_names() -> Vec<(String, String)> {
    vec![
        (
            "a bare name nothing carries".to_string(),
            "echo-absent".to_string(),
        ),
        (
            format!("a bare name at the documented minimum of {TRAIL_NAME_MIN_LEN}"),
            "a9z".to_string(),
        ),
        (
            format!("a bare name at the documented maximum of {TRAIL_NAME_MAX_LEN}"),
            "e".repeat(TRAIL_NAME_MAX_LEN),
        ),
        (
            "a bare name carrying every allowed punctuation and mixed case".to_string(),
            "Echo.a_b-C9".to_string(),
        ),
    ]
}

/// Create `name` in `region` over the wire, as a client does.
async fn create_trail(emulator: &Emulator, region: &str, name: &str) {
    emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({ "Name": name, "S3BucketName": BUCKET }),
        )
        .await
        .expect_ok(&format!("CreateTrail {name} in {region}"));
}

/// Assert `operation` refuses `identifier` as a not-found naming `subject`.
async fn expect_not_found_naming(
    emulator: &Emulator,
    operation: &str,
    request: &Value,
    subject: &str,
    case: &str,
) {
    let account = engine::emulator_account_id();
    let expected = format!("Unknown trail: {subject} for the user: {account}");
    let reply = emulator.call(operation, request).await;
    reply.expect_error(case, NOT_FOUND_STATUS, "TrailNotFoundException");
    assert_eq!(
        reply.message(),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        reply.message()
    );
}

/// The operations that look a trail identifier up name it as the REQUEST wrote it: a
/// bare name stays a bare name, and an ARN keeps every segment the caller sent,
/// including a Region and an account the service would not have derived.
#[tokio::test]
async fn a_name_looked_up_is_echoed_as_the_request_wrote_it() {
    let emulator = Emulator::new();
    // One stored trail, so a refusal is "not this trail" and not "no trails at all".
    create_trail(&emulator, HOME_REGION, "echo-present").await;

    // (case, the identifier, whether a write may take this row) — a write runs in the
    // trail's home Region only, so an ARN from elsewhere is a home-Region refusal for
    // it rather than a not-found.
    let mut rows: Vec<(String, String, bool)> = absent_names()
        .into_iter()
        .map(|(case, name)| (case, name, true))
        .collect();
    rows.push((
        "a trail ARN in the caller's own Region".to_string(),
        trail_arn(HOME_REGION, "echo-absent"),
        true,
    ));
    rows.push((
        "a trail ARN carrying another account's id".to_string(),
        format!("arn:aws:cloudtrail:{HOME_REGION}:{OTHER_ACCOUNT}:trail/echo-absent"),
        true,
    ));
    rows.push((
        "a trail ARN whose name sits at the documented maximum".to_string(),
        trail_arn(HOME_REGION, &"e".repeat(TRAIL_NAME_MAX_LEN)),
        true,
    ));
    // A trail ARN pins its own Region, so a read made elsewhere resolves THAT Region's
    // trail — and the refusal keeps the Region segment the request sent.
    for region in ["eu-west-1", "ap-south-1", "us-west-2"] {
        rows.push((
            format!("a trail ARN in {region}, naming the trail stored here"),
            trail_arn(region, "echo-present"),
            false,
        ));
    }

    for (case, identifier, writes_too) in rows {
        for (operation, mutating, request) in name_echoing_requests(&identifier) {
            if mutating && !writes_too {
                continue;
            }
            let case = format!("{operation} with {case}");
            expect_not_found_naming(&emulator, operation, &request, &identifier, &case).await;
        }
    }
}

/// The operations that resolve the identifier to one Region's trail in order to drive
/// or configure it name that trail's fully-qualified ARN, which is what a caller who
/// passed a bare name is told. Asserted beside the rule above so the two subjects
/// cannot drift into each other.
#[tokio::test]
async fn a_trail_driven_in_its_region_is_named_by_its_resolved_arn() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "echo-present").await;

    for (case, name) in absent_names() {
        // A bare name names the trail of that name in the Region the call was made in.
        let resolved = trail_arn(HOME_REGION, &name);
        for (operation, request) in arn_naming_requests(&name) {
            let case = format!("{operation} with {case}");
            expect_not_found_naming(&emulator, operation, &request, &resolved, &case).await;
        }
        // The ARN form of the same absent trail resolves to itself, so both forms of
        // one identifier are refused with one subject.
        for (operation, request) in arn_naming_requests(&resolved) {
            let case = format!("{operation} with the ARN form of {case}");
            expect_not_found_naming(&emulator, operation, &request, &resolved, &case).await;
        }
    }
}

/// A refusal is the whole answer: an operation refused for an unknown trail did no
/// work, and the trail that IS stored answers the same operations and reports what
/// they wrote.
#[tokio::test]
async fn a_refused_operation_leaves_the_registry_as_it_was() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "echo-present").await;

    // Drive every operation at an identifier naming nothing, writes included.
    for (case, name) in absent_names() {
        for (operation, _mutating, request) in name_echoing_requests(&name) {
            let case = format!("{operation} with {case}");
            expect_not_found_naming(&emulator, operation, &request, &name, &case).await;
        }
        let resolved = trail_arn(HOME_REGION, &name);
        for (operation, request) in arn_naming_requests(&name) {
            let case = format!("{operation} with {case}");
            expect_not_found_naming(&emulator, operation, &request, &resolved, &case).await;
        }
    }

    // Nothing was minted for any refused identifier: the account still holds exactly
    // the one trail that was created.
    let listed = emulator.call("ListTrails", &json!({})).await;
    let names: Vec<String> = listed
        .expect_ok("ListTrails after the refusals")["Trails"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["Name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec!["echo-present".to_string()],
        "case ListTrails after the refusals: expected only the created trail, got {names:?}"
    );

    // The stored trail is untouched by them: a refused StartLogging moved no logging
    // flag.
    let status = emulator
        .call("GetTrailStatus", &json!({ "Name": "echo-present" }))
        .await;
    let status = status.expect_ok("GetTrailStatus echo-present").clone();
    assert_eq!(
        status["IsLogging"],
        json!(false),
        "case GetTrailStatus echo-present: expected a trail nobody started logging on, \
         got {status:?}"
    );

    // Round trip: the write refused for every unknown identifier is served for the
    // stored one, in both forms of its identifier, and reading the trail back reports
    // what it wrote.
    for identifier in [
        "echo-present".to_string(),
        trail_arn(HOME_REGION, "echo-present"),
    ] {
        let written = emulator
            .call(
                "PutEventConfiguration",
                &json!({ "TrailName": identifier, "AggregationConfigurations": aggregations() }),
            )
            .await;
        written.expect_ok(&format!("PutEventConfiguration {identifier}"));
        let read = emulator
            .call("GetEventConfiguration", &json!({ "TrailName": identifier }))
            .await;
        let read = read
            .expect_ok(&format!("GetEventConfiguration {identifier}"))
            .clone();
        assert_eq!(
            read["AggregationConfigurations"],
            aggregations(),
            "case GetEventConfiguration {identifier}: expected the configuration \
             PutEventConfiguration stored, got {read:?}"
        );
        assert_eq!(
            read["TrailARN"],
            json!(trail_arn(HOME_REGION, "echo-present")),
            "case GetEventConfiguration {identifier}: expected the stored trail's ARN, \
             got {read:?}"
        );
    }

    // Deleted, the identifier that used to resolve is refused like any other unknown
    // one — the distinction a caller could not draw while an unknown trail answered
    // success.
    emulator
        .call("DeleteTrail", &json!({ "Name": "echo-present" }))
        .await
        .expect_ok("DeleteTrail echo-present");
    expect_not_found_naming(
        &emulator,
        "GetEventConfiguration",
        &json!({ "TrailName": "echo-present" }),
        "echo-present",
        "GetEventConfiguration after DeleteTrail",
    )
    .await;
}
