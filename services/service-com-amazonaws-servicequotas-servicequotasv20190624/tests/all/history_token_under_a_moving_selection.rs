// SPDX-License-Identifier: CC0-1.0

//! What a change-history `NextToken` keeps meaning while the SELECTION it was cut from
//! moves under the caller.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour. The harness (`Client`, `client`, `emulator`) is the one
//! `quota_workflow.rs` owns, and the quota codes are looked up through `quota_code_for`
//! rather than written down.
//!
//! The RULE, asserted over a TABLE of selections and mutations rather than over one
//! recorded call: a history page token names the position the page stopped at in the
//! listing's own ORDER — the order key of the last record it reported — and not a count
//! of the records ahead of it. So between two pages of one walk a record may be STORED
//! (a further increase, which the newest-first history reports ahead of everything
//! already seen) and a record may LEAVE the selection (the clock decides a request, so
//! it no longer carries the `Status` the selection asked for), and the walk still
//! resumes at the position its token named: it reports the records that sit after that
//! position, each at most once, and the token stays good even when nothing is left after
//! it. Counting records ahead of the page cannot do that — a record stored ahead of the
//! boundary reports one record on two pages, and a record leaving the selection moves
//! the count past a record that is then reported on no page at all.
//!
//! The rows span both history listings (`ListRequestedServiceQuotaChangeHistory` and
//! `ListRequestedServiceQuotaChangeHistoryByQuota`), both directions the selection can
//! move (records added, records leaving), the two page sizes a boundary can fall at, and
//! the two planes a request is made on (the account's and one resource's).
//!
//! `pagination_token_position.rs` pins the same rule for the record set of a listing as
//! a whole; what this suite adds is the selection a `Status` filter cuts out of it,
//! which SHRINKS as the clock settles the requests it holds.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    LEVEL_ALL, REQUEST_DECISION_DELAY, STATUS_APPROVED, STATUS_PENDING, SUPPORT_CASE_OPEN_DELAY,
};

use crate::quota_workflow::{client, emulator, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// The account-wide change history, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The per-quota change history.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// The output member both history listings carry their records in.
const RECORDS: &str = "RequestedQuotas";

/// The endpoint every row calls.
const REGION: &str = "us-east-1";

/// How often a selection is read while the clock settles the requests it holds: well
/// inside the shortest step the clock owes, so a step is not stepped over.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The quota whose per-resource plane a row walks: AWS publishes it as adjustable per
/// resource, so each resource is a plane of its own and several of its requests can be
/// open at once (`src/state.rs`, the `es` rows).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// A quota adjustable at the account level, raised again and again by the per-quota
/// rows.
const ACCOUNT_QUOTA_NAME: &str = "VPCs per Region";

/// A deadline that clears BOTH steps the clock owes a quota increase request
/// (`SUPPORT_CASE_OPEN_DELAY`, then `REQUEST_DECISION_DELAY` once its support case is
/// open) many times over, so it only ever fires on a request that is genuinely stuck.
fn settling_deadline() -> Instant {
    Instant::now() + 20 * (SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY)
}

/// One increase to ask for: the quota, the value, and the resource plane it is asked on.
#[derive(Clone, Debug)]
struct Increase {
    service_code: String,
    quota_code: String,
    desired: f64,
    context_id: Option<String>,
}

impl Increase {
    /// An increase on the account's own plane.
    fn account(service_code: &str, quota_code: &str, desired: f64) -> Self {
        Self {
            service_code: service_code.to_string(),
            quota_code: quota_code.to_string(),
            desired,
            context_id: None,
        }
    }

    /// An increase on one resource's plane.
    fn on_resource(service_code: &str, quota_code: &str, desired: f64, context_id: &str) -> Self {
        Self {
            service_code: service_code.to_string(),
            quota_code: quota_code.to_string(),
            desired,
            context_id: Some(context_id.to_string()),
        }
    }
}

/// A domain ARN of this account, as a `ContextId` naming one resource.
fn domain(name: &str) -> String {
    format!("arn:aws:es:{REGION}:{ACCOUNT}:domain/{name}")
}

/// Ask for `increase` and return the `Id` of the request it minted.
async fn raise(caller: &Client, increase: &Increase) -> String {
    let mut body = json!({
        "ServiceCode": increase.service_code,
        "QuotaCode": increase.quota_code,
        "DesiredValue": increase.desired,
    });
    if let Some(context_id) = &increase.context_id {
        body["ContextId"] = json!(context_id);
    }
    let response = caller.ok("RequestServiceQuotaIncrease", body).await;
    response["RequestedQuota"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {increase:?}: expected a minted Id, got {response}"))
        .to_string()
}

/// The `Status` the single read of one request reports.
async fn status_of(caller: &Client, id: &str) -> String {
    let response = caller
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await;
    response["RequestedQuota"]["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {id}: expected a Status, got {response}"))
        .to_string()
}

/// Ask for `increase` and wait — by READING the request, never by assuming a duration —
/// until the clock has carried it to its decision, so the selection it then belongs to
/// is a terminal one that no later step moves it out of.
async fn raise_and_await_the_decision(caller: &Client, increase: &Increase) -> String {
    let id = raise(caller, increase).await;
    let deadline = settling_deadline();
    loop {
        let status = status_of(caller, &id).await;
        if status == STATUS_APPROVED {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "case {increase:?}: expected the request to reach {STATUS_APPROVED}, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `Id`s one page carries, in page order.
fn ids(page: &Value) -> Vec<String> {
    page[RECORDS]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            record["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case a history record: expected an Id, got {record}"))
                .to_string()
        })
        .collect()
}

/// The WHOLE selection `body` names, which is what a call naming no `MaxResults` serves.
async fn selection(caller: &Client, operation: &str, body: &Value) -> Vec<String> {
    let served = caller.ok(operation, body.clone()).await;
    assert_eq!(
        served["NextToken"],
        Value::Null,
        "case {operation} {body}: a call naming no MaxResults serves the whole selection, \
         got {served}"
    );
    ids(&served)
}

/// `body` with the page size and, when there is one, the token to resume at.
fn asked(body: &Value, page_size: i64, token: Option<&str>) -> Value {
    let mut asked = body.clone();
    asked["MaxResults"] = json!(page_size);
    if let Some(token) = token {
        asked["NextToken"] = json!(token);
    }
    asked
}

/// The first page of `body`, and the token that resumes after it.
async fn first_page(
    case: &str,
    caller: &Client,
    operation: &str,
    body: &Value,
    page_size: i64,
) -> (Vec<String>, String) {
    let served = caller.ok(operation, asked(body, page_size, None)).await;
    let page = ids(&served);
    let token = served["NextToken"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: records remain after a page of {page_size}, so a NextToken must \
                 follow it, got {served}"
            )
        });
    (page, token)
}

/// Walk `body` from `token` to the end, asserting the page contract on the way, and
/// return every `Id` the rest of the walk reported.
async fn rest_of_the_walk(
    case: &str,
    caller: &Client,
    operation: &str,
    body: &Value,
    page_size: i64,
    token: &str,
) -> Vec<String> {
    let mut token = token.to_string();
    let mut served: Vec<String> = Vec::new();
    // Bounded so a walk that never reports a last page fails instead of hanging.
    for _ in 0..64 {
        let page = caller
            .ok(operation, asked(body, page_size, Some(&token)))
            .await;
        let reported = ids(&page);
        assert!(
            reported.len() as i64 <= page_size,
            "case {case}: a page holds at most {page_size} records, got {reported:?}"
        );
        served.extend(reported);
        match page["NextToken"].as_str() {
            Some(next) => token = next.to_string(),
            None => return served,
        }
    }
    panic!("case {case}: the walk must reach a page with no NextToken, got {served:?}");
}

/// Assert that nothing was reported twice over one walk.
fn assert_each_record_once(case: &str, walked: &[String]) {
    let distinct: BTreeSet<&String> = walked.iter().collect();
    assert_eq!(
        distinct.len(),
        walked.len(),
        "case {case}: no record may be reported twice in one walk, got {walked:?}"
    );
}

/// One row of the SHRINKING table: a selection held together by a `Status` the clock
/// takes away from the requests it holds.
struct Shrinking {
    /// Why this row is in the table.
    case: &'static str,
    operation: &'static str,
    /// The selection walked, without its page size.
    body: Value,
    /// The increases to ask for, OLDEST FIRST — the reverse of the order the listing
    /// reports them in.
    seeds: Vec<Increase>,
    /// The page size the walk asks for, so the boundary falls at a different record in
    /// each row.
    page_size: i64,
}

/// One row of the GROWING table: a selection of DECIDED requests, which no later step
/// moves a record out of, with a further increase asked for mid-walk.
struct Growing {
    /// Why this row is in the table.
    case: &'static str,
    operation: &'static str,
    /// The selection walked, without its page size.
    body: Value,
    /// The increases to ask for and let the clock decide, OLDEST FIRST.
    seeds: Vec<Increase>,
    /// The increase asked for between the first page and the rest of the walk; decided
    /// too, so it joins the same selection — ahead of every record already reported,
    /// the history being newest first.
    mutation: Increase,
    page_size: i64,
}

/// The rows whose selection SHRINKS: the `PENDING` requests of the account, and the
/// `PENDING` requests on one quota's resource planes (one plane per resource, so several
/// of that quota's requests are open at once).
async fn shrinking_rows(caller: &Client) -> Vec<Shrinking> {
    let resource_quota = quota_code_for(caller, "es", RESOURCE_QUOTA_NAME).await;
    vec![
        Shrinking {
            case: "the account's PENDING requests, paged one at a time",
            operation: HISTORY,
            body: json!({"Status": STATUS_PENDING}),
            seeds: vec![
                // Distinct adjustable quotas of the seeded catalog, each raised above
                // its published default, because one plane holds one open request.
                Increase::account("vpc", "L-F678F1CE", 10.0),
                Increase::account("lambda", "L-B99A9384", 2_000.0),
                Increase::account("dynamodb", "L-F98FE922", 5_000.0),
            ],
            page_size: 1,
        },
        Shrinking {
            case: "one quota's PENDING requests on three resource planes, paged two at a time",
            operation: HISTORY_BY_QUOTA,
            body: json!({
                "ServiceCode": "es",
                "QuotaCode": resource_quota,
                "Status": STATUS_PENDING,
                "QuotaRequestedAtLevel": LEVEL_ALL,
            }),
            seeds: vec![
                Increase::on_resource("es", &resource_quota, 120.0, &domain("alpha")),
                Increase::on_resource("es", &resource_quota, 130.0, &domain("beta")),
                Increase::on_resource("es", &resource_quota, 140.0, &domain("gamma")),
            ],
            page_size: 2,
        },
    ]
}

/// The rows whose selection GROWS ahead of the boundary: the account's `APPROVED`
/// requests, and one quota's — the same quota raised again and again, each increase
/// decided before the next is asked for.
async fn growing_rows(caller: &Client) -> Vec<Growing> {
    let account_quota = quota_code_for(caller, "vpc", ACCOUNT_QUOTA_NAME).await;
    vec![
        Growing {
            case: "the account's APPROVED requests, paged one at a time",
            operation: HISTORY,
            body: json!({"Status": STATUS_APPROVED}),
            seeds: vec![
                Increase::account("vpc", "L-F678F1CE", 10.0),
                Increase::account("lambda", "L-B99A9384", 2_000.0),
                Increase::account("dynamodb", "L-F98FE922", 5_000.0),
            ],
            mutation: Increase::account("s3", "L-DC2B2D3D", 20_000.0),
            page_size: 1,
        },
        Growing {
            case: "one quota's APPROVED requests, paged two at a time",
            operation: HISTORY_BY_QUOTA,
            body: json!({
                "ServiceCode": "vpc",
                "QuotaCode": account_quota,
                "Status": STATUS_APPROVED,
            }),
            seeds: vec![
                // Each increase has to be above the value the one before it put in
                // force.
                Increase::account("vpc", &account_quota, 10.0),
                Increase::account("vpc", &account_quota, 20.0),
                Increase::account("vpc", &account_quota, 30.0),
            ],
            mutation: Increase::account("vpc", &account_quota, 40.0),
            page_size: 2,
        },
    ]
}

/// RULE: a token names the position the page stopped at in the listing's order, so a
/// selection that SHRINKS between two pages leaves the token naming that same position:
/// the resumed page carries the records still sitting after it — none here, every
/// request having been decided — and is a well-formed final page rather than a refusal.
#[tokio::test]
async fn a_shrinking_status_selection_leaves_the_token_at_the_position_it_was_cut_at() {
    for row in shrinking_rows(&client(&emulator(), REGION)).await {
        // One emulator per row, so each row's selection holds only its own requests.
        let caller = client(&emulator(), REGION);
        let case = row.case;

        let mut minted: Vec<String> = Vec::new();
        for seed in &row.seeds {
            minted.push(raise(&caller, seed).await);
        }
        let newest_first: Vec<String> = minted.iter().rev().cloned().collect();
        let held = selection(&caller, row.operation, &row.body).await;
        assert_eq!(
            held, newest_first,
            "case {case}: the selection must hold the requests just made, newest first, \
             got {held:?}"
        );

        let (page_one, token) =
            first_page(case, &caller, row.operation, &row.body, row.page_size).await;
        assert_eq!(
            page_one,
            newest_first[..row.page_size as usize].to_vec(),
            "case {case}: the first page must be the first {} records of the selection, \
             got {page_one:?}",
            row.page_size
        );

        // The selection empties as the clock decides the requests it held — not after a
        // wait of some length, but when reading it says so.
        let deadline = settling_deadline();
        loop {
            let still_held = selection(&caller, row.operation, &row.body).await;
            if still_held.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected every request to be decided and leave the selection, \
                 got {still_held:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // The rule: the token still names the position the first page stopped at, so
        // the walk ends there — and the records that left the selection are reported on
        // no page of it, rather than shifting it onto a record it never reports.
        let rest = rest_of_the_walk(
            case,
            &caller,
            row.operation,
            &row.body,
            row.page_size,
            &token,
        )
        .await;
        assert!(
            rest.is_empty(),
            "case {case}: nothing is left in the selection after the position the token \
             named, got {rest:?}"
        );
        let walked: Vec<String> = page_one.iter().chain(rest.iter()).cloned().collect();
        assert_each_record_once(case, &walked);

        // The records left the SELECTION, not the history: the same listing with no
        // `Status` filter still reports every one of them.
        let mut unfiltered = row.body.clone();
        unfiltered
            .as_object_mut()
            .expect("the selection body is an object")
            .remove("Status");
        let history = selection(&caller, row.operation, &unfiltered).await;
        assert_eq!(
            history, newest_first,
            "case {case}: every request must still be reported by the unfiltered history, \
             got {history:?}"
        );
    }
}

/// RULE, the other direction: a record stored between two pages — which the newest-first
/// history reports AHEAD of everything already seen — leaves the walk resuming at the
/// position its token named, so the rest of the walk is exactly the records after that
/// position, each reported once, and the record stored mid-walk is reported on no page of
/// it.
#[tokio::test]
async fn a_record_stored_mid_walk_does_not_move_the_position_a_token_names() {
    for row in growing_rows(&client(&emulator(), REGION)).await {
        // One emulator per row, so each row's selection holds only its own requests.
        let caller = client(&emulator(), REGION);
        let case = row.case;

        let mut minted: Vec<String> = Vec::new();
        for seed in &row.seeds {
            minted.push(raise_and_await_the_decision(&caller, seed).await);
        }
        let newest_first: Vec<String> = minted.iter().rev().cloned().collect();
        let held = selection(&caller, row.operation, &row.body).await;
        assert_eq!(
            held, newest_first,
            "case {case}: the selection must hold the decided requests, newest first, \
             got {held:?}"
        );

        let (page_one, token) =
            first_page(case, &caller, row.operation, &row.body, row.page_size).await;
        assert_eq!(
            page_one,
            newest_first[..row.page_size as usize].to_vec(),
            "case {case}: the first page must be the first {} records of the selection, \
             got {page_one:?}",
            row.page_size
        );

        // One more increase, decided like the others, so the selection gains a record
        // ahead of every record the first page reported.
        let stored_mid_walk = raise_and_await_the_decision(&caller, &row.mutation).await;
        let grown = selection(&caller, row.operation, &row.body).await;
        let mut expected = vec![stored_mid_walk.clone()];
        expected.extend(newest_first.clone());
        assert_eq!(
            grown, expected,
            "case {case}: the request made mid-walk must be reported first, got {grown:?}"
        );

        let rest = rest_of_the_walk(
            case,
            &caller,
            row.operation,
            &row.body,
            row.page_size,
            &token,
        )
        .await;
        let walked: Vec<String> = page_one.iter().chain(rest.iter()).cloned().collect();
        assert_eq!(
            walked, newest_first,
            "case {case}: the walk must report the records that sat after the position its \
             token named, got {walked:?}"
        );
        assert!(
            !walked.contains(&stored_mid_walk),
            "case {case}: the request made after the token was issued sits ahead of it and \
             belongs to no page of this walk, got {walked:?}"
        );
        assert_each_record_once(case, &walked);
    }
}
