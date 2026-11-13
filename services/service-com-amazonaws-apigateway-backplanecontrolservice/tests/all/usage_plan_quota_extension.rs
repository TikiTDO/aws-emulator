// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GRANT of a remaining quota: `UpdateUsage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service, and the `execute-api` data plane on the
//! api's own invoke host — so what is asserted is what a client sees, not what a handler
//! returns. The emulator, the metered api, the API keys, the metering plan and the report
//! reader are the harness the sibling suite over the usage report
//! (`usage_plan_usage_report`) already stands up.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * `UpdateUsage` "grants a temporary extension to the remaining quota of a usage plan
//!   associated with a specified API key" (`smithy_query(query="operation",
//!   operation="UpdateUsage")`): a `replace` of `/remaining` states what the metering period
//!   that holds the call has LEFT for that `(usage plan, API key)` pair, so from then on the
//!   period admits exactly that many further requests from the key and the next one is
//!   refused — over a table spanning the metering period, the plan's limit and offset, how
//!   much the key had already sent, whether the grant is above or below what the period had
//!   left, and which op names the pointer;
//! * the grant is what the report reads back: the answer carries the key's daily log for the
//!   day the grant was made on, and a later `GetUsage` over that day reports the same pair of
//!   numbers — one accounting, written by the grant and read by both;
//! * the extension is TEMPORARY, so it covers only the period it was granted in: every later
//!   period starts at the plan's own `quota` limit again;
//! * a grant states the remaining quota outright, so a second grant replaces the first rather
//!   than compounding with it, and a request carrying no patch operation grants nothing;
//! * a `usagePlanId` naming no stored plan and a `keyId` naming no API key the plan meters are
//!   refused 404, and a patch the record cannot take — a pointer it publishes no member for, an
//!   op it publishes no pointer for, a value that is no count of requests — and a plan that
//!   meters no quota at all are refused 400, each leaving the accounting exactly as it was.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    invalid_quota_count, INVALID_PATCH_OPERATION, NO_SUCH_METERED_API_KEY, NO_SUCH_USAGE_PLAN,
    NO_USAGE_PLAN_QUOTA,
};

use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};
use crate::usage_plan_usage_report::{
    api_key, date, get_usage, invoke, log_of, metered_api, metering_plan, plus, send_one, today,
    LIMIT_EXCEEDED,
};

/// `UpdateUsage` for the `(plan, key)` pair, carrying `patches` verbatim as its
/// `patchOperations`.
async fn update_usage(gateway: &Gateway, plan: &str, key_id: &str, patches: Value) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{plan}/keys/{key_id}/usage"),
            Some(json!({ "patchOperations": patches })),
        )
        .await
}

/// One `patchOperations` entry naming the remaining quota of the metering period.
fn remaining_patch(op: &str, value: i64) -> Value {
    json!([{ "op": op, "path": "/remaining", "value": value.to_string() }])
}

/// The daily log the ANSWER of a grant carries for `key_id`, with the days it covers.
fn granted_log(answer: &Answer, key_id: &str, case: &str) -> (String, String, Vec<Vec<i64>>) {
    (
        answer.body["startDate"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a startDate, got {:?}", answer.body))
            .to_string(),
        answer.body["endDate"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected an endDate, got {:?}", answer.body))
            .to_string(),
        log_of(&answer.body, key_id, case),
    )
}

/// One row of the grant table: the quota the plan meters, how much the key had sent before
/// the grant, and the remaining quota the grant states.
struct Row {
    case: &'static str,
    /// The metering period of the plan's quota, from the modelled `QuotaPeriodType`
    /// (`smithy_query(query="shape", shape="QuotaPeriodType")`: DAY, WEEK, MONTH).
    period: &'static str,
    /// The plan's `quota` limit and `offset`.
    limit: i64,
    offset: i64,
    /// How many requests the key sends before the grant; every one of them is admitted.
    sent: i64,
    /// The `op` of the `patchOperations` entry: both ops that SET a single-valued member.
    op: &'static str,
    /// The remaining quota the grant states — below, above, or past the period's own budget.
    granted: i64,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a daily quota, granted less than it had left",
            period: "DAY",
            limit: 10,
            offset: 0,
            sent: 1,
            op: "replace",
            granted: 3,
        },
        Row {
            case: "a daily quota, granted nothing at all",
            period: "DAY",
            limit: 10,
            offset: 0,
            sent: 2,
            op: "replace",
            granted: 0,
        },
        Row {
            case: "a weekly quota with an offset, granted past its own limit",
            period: "WEEK",
            limit: 5,
            offset: 2,
            sent: 1,
            op: "replace",
            granted: 4,
        },
        Row {
            case: "a monthly quota already exhausted, granted a fresh extension",
            period: "MONTH",
            limit: 1,
            offset: 0,
            sent: 1,
            op: "replace",
            granted: 2,
        },
        Row {
            case: "a daily quota and an add of the same pointer, nothing sent yet",
            period: "DAY",
            limit: 4,
            offset: 0,
            sent: 0,
            op: "add",
            granted: 1,
        },
    ]
}

#[tokio::test]
async fn a_grant_states_what_the_period_has_left_and_the_data_plane_admits_against_it() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-grant-table").await;
    let day = date(today());

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (key_id, key_value) = api_key(&gateway, case, index).await;
        let plan = metering_plan(
            &gateway,
            case,
            Some(json!({
                "limit": row.limit,
                "offset": row.offset,
                "period": row.period,
            })),
            &api,
            ::std::slice::from_ref(&key_id),
        )
        .await;
        for _ in 0..row.sent {
            send_one(&gateway, &api, &key_value, case).await;
        }

        // The grant: what the period has left for this key from now on.
        let granted = update_usage(
            &gateway,
            &plan,
            &key_id,
            remaining_patch(row.op, row.granted),
        )
        .await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected the grant to be served, got {} with {:?}",
            granted.status, granted.body
        );
        assert_eq!(
            granted.body["usagePlanId"].as_str(),
            Some(plan.as_str()),
            "case {case}: expected the plan the grant was made on, got {:?}",
            granted.body["usagePlanId"]
        );
        let (start, end, log) = granted_log(&granted, &key_id, case);
        assert_eq!(
            (start.as_str(), end.as_str()),
            (day.as_str(), day.as_str()),
            "case {case}: expected the day the grant was made on, got {start:?} through {end:?}"
        );
        assert_eq!(
            log,
            vec![vec![row.sent, row.granted]],
            "case {case}: expected the key to have sent {} with {} left, got {log:?}",
            row.sent,
            row.granted
        );

        // The same accounting read back through the report: one grant, one pair of numbers.
        let reported = get_usage(&gateway, &plan, &day, &day, "").await;
        assert_eq!(
            reported.status, 200,
            "case {case}: expected the report to be served, got {} with {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            log_of(&reported.body, &key_id, case),
            vec![vec![row.sent, row.granted]],
            "case {case}: expected the report to carry the granted quota, got {:?}",
            reported.body["values"]
        );

        // What the data plane then admits: exactly the granted number of further requests,
        // and the one past it is refused the way every request past a quota is.
        for n in 1..=row.granted {
            let served = invoke(&gateway, &api, &key_value).await;
            assert_eq!(
                served.status, 200,
                "case {case}: expected request {n} of the granted {} to be served, got {} \
                 with {:?}",
                row.granted, served.status, served.body
            );
        }
        let refused = invoke(&gateway, &api, &key_value).await;
        assert_eq!(
            refused.status, 429,
            "case {case}: expected the request past the granted quota to be refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            LIMIT_EXCEEDED,
            "case {case}: expected {LIMIT_EXCEEDED:?} past the granted quota, got {:?}",
            refused.message()
        );

        // And the report says so: everything the key sent that day, with nothing left.
        let spent = get_usage(&gateway, &plan, &day, &day, "").await;
        assert_eq!(
            log_of(&spent.body, &key_id, case),
            vec![vec![row.sent + row.granted, 0]],
            "case {case}: expected the granted quota to be spent, got {:?}",
            spent.body["values"]
        );
    }
}

#[tokio::test]
async fn a_grant_covers_the_period_it_was_made_in_and_no_later_one() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-grant-periods").await;
    let from = today();

    // One row per metering period, with the number of days that certainly lands in a LATER
    // period of that length: the next day, the same weekday a week on, and a day more than
    // the longest month away.
    for (index, (period, later, limit, granted)) in [
        ("DAY", 1_u64, 7_i64, 99_i64),
        ("WEEK", 7, 500, 2),
        ("MONTH", 31, 1_000, 4_000),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("a {period} quota granted {granted} remaining");
        let (key_id, key_value) = api_key(&gateway, &case, 100 + index).await;
        let plan = metering_plan(
            &gateway,
            &case,
            Some(json!({ "limit": limit, "offset": 0, "period": period })),
            &api,
            ::std::slice::from_ref(&key_id),
        )
        .await;
        send_one(&gateway, &api, &key_value, &case).await;
        let answer = update_usage(&gateway, &plan, &key_id, remaining_patch("replace", granted))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the grant to be served, got {} with {:?}",
            answer.status, answer.body
        );

        let through = plus(from, later);
        let reported = get_usage(&gateway, &plan, &date(from), &date(through), "").await;
        let entries = log_of(&reported.body, &key_id, &case);
        assert_eq!(
            entries.first(),
            Some(&vec![1, granted]),
            "case {case}: expected the granted period to have {granted} left, got {:?}",
            entries.first()
        );
        assert_eq!(
            entries.last(),
            Some(&vec![0, limit]),
            "case {case}: expected a later period to start at the plan's own limit, got {:?}",
            entries.last()
        );
    }
}

#[tokio::test]
async fn a_second_grant_replaces_the_first_and_a_grant_of_nothing_changes_nothing() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-grant-replacement").await;
    let day = date(today());
    let case = "a daily quota granted twice";

    let (key_id, key_value) = api_key(&gateway, case, 200).await;
    let plan = metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 20, "offset": 0, "period": "DAY" })),
        &api,
        ::std::slice::from_ref(&key_id),
    )
    .await;
    send_one(&gateway, &api, &key_value, case).await;

    // A grant states the remaining quota outright, so each one is the number the period has
    // from then on — the later grant does not add to the earlier one.
    for granted in [2_i64, 5, 3] {
        let answer =
            update_usage(&gateway, &plan, &key_id, remaining_patch("replace", granted)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the grant of {granted} to be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            log_of(&answer.body, &key_id, case),
            vec![vec![1, granted]],
            "case {case}: expected {granted} left after granting {granted}, got {:?}",
            answer.body["values"]
        );
    }

    // A request carrying no patch operation grants nothing: it reports the usage data as it
    // stands, and the next read says the same. Both spellings of "no operation" — the member
    // left out of the request, and an empty list — are the same request.
    for (case, request) in [
        ("no patchOperations member at all", json!({})),
        ("an empty patchOperations list", json!({ "patchOperations": [] })),
    ] {
        let answer = gateway
            .call(
                "PATCH",
                &format!("/usageplans/{plan}/keys/{key_id}/usage"),
                Some(request),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected a grant of nothing to be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            log_of(&answer.body, &key_id, case),
            vec![vec![1, 3]],
            "case {case}: expected the usage as it stood, got {:?}",
            answer.body["values"]
        );
    }
    let reported = get_usage(&gateway, &plan, &day, &day, "").await;
    assert_eq!(
        log_of(&reported.body, &key_id, case),
        vec![vec![1, 3]],
        "case {case}: expected the last grant to be the one that stands, got {:?}",
        reported.body["values"]
    );
}

#[tokio::test]
async fn an_unresolvable_pair_an_unapplicable_patch_or_a_plan_with_no_quota_is_refused() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-grant-refusals").await;
    let day = date(today());
    let case = "the plan every row grants on";

    let (key_id, key_value) = api_key(&gateway, case, 300).await;
    let (unmetered, _) = api_key(&gateway, case, 301).await;
    let plan = metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 10, "offset": 0, "period": "DAY" })),
        &api,
        ::std::slice::from_ref(&key_id),
    )
    .await;
    send_one(&gateway, &api, &key_value, case).await;

    // The pair the grant is made on: each half names a record, so each is refused as the
    // record it names.
    for (case, plan_id, key, message) in [
        (
            "a usagePlanId naming no stored plan",
            "no-such-plan".to_string(),
            key_id.clone(),
            NO_SUCH_USAGE_PLAN,
        ),
        (
            "a keyId naming no stored key at all",
            plan.clone(),
            "no-such-key".to_string(),
            NO_SUCH_METERED_API_KEY,
        ),
        (
            "a keyId naming a stored key the plan does not meter",
            plan.clone(),
            unmetered.clone(),
            NO_SUCH_METERED_API_KEY,
        ),
    ] {
        let refused = update_usage(&gateway, &plan_id, &key, remaining_patch("replace", 5)).await;
        assert_failure(&refused, 404, "NotFoundException", case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(message),
            "case {case}: expected {message:?}, got {:?}",
            refused.body["message"]
        );
    }

    // The patch itself: a pointer the record publishes no member for, an op it publishes no
    // pointer for, and a value that is no count of requests.
    for (case, patches, message) in [
        (
            "a pointer naming no member of the record",
            json!([{ "op": "replace", "path": "/limit", "value": "5" }]),
            "Invalid patch path  '/limit' specified for op 'replace'. Must be one of: \
             [/remaining]"
                .to_string(),
        ),
        (
            "a remove of the remaining quota",
            json!([{ "op": "remove", "path": "/remaining" }]),
            INVALID_PATCH_OPERATION.to_string(),
        ),
        (
            "a negative count of requests",
            remaining_patch("replace", -1),
            invalid_quota_count("remaining"),
        ),
        (
            "a value that is no number",
            json!([{ "op": "replace", "path": "/remaining", "value": "many" }]),
            "/remaining takes an integer value, got \"many\"".to_string(),
        ),
    ] {
        let refused = update_usage(&gateway, &plan, &key_id, patches).await;
        assert_failure(&refused, 400, "BadRequestException", case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(message.as_str()),
            "case {case}: expected {message:?}, got {:?}",
            refused.body["message"]
        );
    }

    // Every refusal above left the accounting exactly as it was: the one request this key
    // sent, against the plan's own limit.
    let unchanged = get_usage(&gateway, &plan, &day, &day, "").await;
    assert_eq!(
        log_of(&unchanged.body, &key_id, case),
        vec![vec![1, 9]],
        "case {case}: expected a refused grant to change nothing, got {:?}",
        unchanged.body["values"]
    );

    // A plan metering no quota has no remaining quota a grant could state.
    let (unquotaed_key, _) = api_key(&gateway, case, 302).await;
    let unquotaed = metering_plan(
        &gateway,
        "a plan with no quota at all",
        None,
        &api,
        ::std::slice::from_ref(&unquotaed_key),
    )
    .await;
    let refused = update_usage(
        &gateway,
        &unquotaed,
        &unquotaed_key,
        remaining_patch("replace", 5),
    )
    .await;
    assert_failure(
        &refused,
        400,
        "BadRequestException",
        "a plan with no quota at all",
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some(NO_USAGE_PLAN_QUOTA),
        "case a plan with no quota at all: expected {NO_USAGE_PLAN_QUOTA:?}, got {:?}",
        refused.body["message"]
    );
}
