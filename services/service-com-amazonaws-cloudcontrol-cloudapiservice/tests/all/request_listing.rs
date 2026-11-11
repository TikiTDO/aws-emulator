// SPDX-License-Identifier: CC0-1.0

//! `ListResourceRequests`: which resource operation requests a listing reports, in what
//! order, how it is narrowed by the filter criteria, and how a page resumes where the
//! previous one stopped.
//!
//! The rule under test: a listing reports the request records this service holds — the
//! same records `GetResourceRequestStatus` resolves one token to — newest first by the
//! instant each was initiated, narrowed to the operation types and operation statuses the
//! filter names, and paged by `MaxResults` with a continuation token this listing minted.

use std::time::Duration;

use serde_json::{json, Map, Value};

use super::emulator::{code, message, progress, text, Emulator, STATUS_IN_PROGRESS, STATUS_SUCCESS};

/// The type most cases drive requests against.
const STREAM: &str = "AWS::Kinesis::Stream";

/// A second type, so a listing that reports requests of both is not one type's listing.
const RULE: &str = "AWS::Events::Rule";

/// The `Operation` values the model's own enum declares
/// (`smithy_query(query="shape",shape="Operation")`).
const OPERATION_CREATE: &str = "CREATE";
const OPERATION_UPDATE: &str = "UPDATE";
const OPERATION_DELETE: &str = "DELETE";

/// An `OperationStatus` the model declares that no request of this emulator reaches, so a
/// filter naming it admits nothing (`smithy_query(query="shape",shape="OperationStatus")`).
const STATUS_CANCEL_COMPLETE: &str = "CANCEL_COMPLETE";

/// The `OperationStatus` values the model declares
/// (`smithy_query(query="shape",shape="OperationStatus")`), in the order this service's
/// front door lists a value set in — the order its own table of the admissible values is
/// walked, which is neither the model's declaration order nor a sorted one.
const OPERATION_STATUS_VALUES: &str =
    "CANCEL_IN_PROGRESS, SUCCESS, IN_PROGRESS, PENDING, FAILED, CANCEL_COMPLETE";

/// The `Operation` values the model declares
/// (`smithy_query(query="shape",shape="Operation")`), in that same order.
const OPERATION_VALUES: &str = "UPDATE, DELETE, CREATE";

/// Long enough that two calls made around it carry different `EventTime`s, so a case about
/// ORDER does not depend on the clock's resolution.
const APART: Duration = Duration::from_millis(5);

/// One row of the filter table: what the case is, the operation types and statuses the
/// listing's criteria name, and the requests those criteria admit, newest first.
type FilterRow<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a String>);

/// One row of the enum-refusal table: what the case is, the criteria the listing carries,
/// the list the clause echoes, and the member it is stated about.
type EnumRow<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, &'a str, &'a str);

/// One `ListResourceRequests` request body.
fn list_body(
    max_results: Option<i64>,
    next_token: Option<&str>,
    operations: &[&str],
    statuses: &[&str],
) -> Value {
    let mut request = Map::new();
    if let Some(max_results) = max_results {
        request.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(token) = next_token {
        request.insert("NextToken".to_string(), json!(token));
    }
    if !operations.is_empty() || !statuses.is_empty() {
        let mut filter = Map::new();
        if !operations.is_empty() {
            filter.insert("Operations".to_string(), json!(operations));
        }
        if !statuses.is_empty() {
            filter.insert("OperationStatuses".to_string(), json!(statuses));
        }
        request.insert(
            "ResourceRequestStatusFilter".to_string(),
            Value::Object(filter),
        );
    }
    Value::Object(request)
}

/// The `ResourceRequestStatusSummaries` of a listing.
fn summaries(listed: &Value) -> Vec<Value> {
    listed["ResourceRequestStatusSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The request tokens a listing reports, in the order it reported them.
fn reported(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| text(summary, "RequestToken"))
        .collect()
}

/// One `ListResourceRequests` call, required to succeed.
async fn list(
    emulator: &Emulator,
    max_results: Option<i64>,
    next_token: Option<&str>,
    operations: &[&str],
    statuses: &[&str],
) -> Value {
    emulator
        .cc_ok(
            "ListResourceRequests",
            list_body(max_results, next_token, operations, statuses),
        )
        .await
}

/// Every request token the listing reports, walking the pages at `max_results` per page,
/// in the order the pages reported them.
async fn walk(
    emulator: &Emulator,
    max_results: Option<i64>,
    operations: &[&str],
    statuses: &[&str],
) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let listed = list(emulator, max_results, next.as_deref(), operations, statuses).await;
        tokens.extend(reported(&listed));
        match listed["NextToken"].as_str() {
            Some(token) => next = Some(token.to_string()),
            None => return tokens,
        }
        assert!(
            tokens.len() <= 1_000,
            "page size {max_results:?}: the pages never ran out, got {tokens:?}"
        );
    }
}

/// Start one create and wait until it has completed, answering its request token.
async fn created(emulator: &Emulator, type_name: &str, desired_state: &Value) -> String {
    let accepted = emulator.create_ok(type_name, desired_state, None).await;
    let token = text(&accepted, "RequestToken");
    emulator.settled(&token).await;
    token
}

#[tokio::test]
async fn a_listing_reports_every_request_newest_first_and_agrees_with_the_status_call() {
    let emulator = Emulator::new();
    // One request of each operation type, over two resource types, each initiated a
    // measurable instant after the one before it — so the order a listing reports cannot
    // be insertion order of the store's own map by accident.
    let mut accepted = Vec::new();
    accepted.push(created(&emulator, STREAM, &json!({ "StreamName": "alpha" })).await);
    tokio::time::sleep(APART).await;
    accepted.push(created(&emulator, RULE, &json!({ "Name": "zulu" })).await);
    tokio::time::sleep(APART).await;
    let updated = emulator
        .update(
            STREAM,
            "alpha",
            &json!([{ "op": "add", "path": "/Stage", "value": "prod" }]),
            None,
        )
        .await;
    assert_eq!(
        updated.0, 200,
        "UpdateResource: expected HTTP 200, got {} with body {:?}",
        updated.0, updated.1
    );
    let update_token = text(&progress(&updated.1), "RequestToken");
    emulator.settled(&update_token).await;
    accepted.push(update_token);
    tokio::time::sleep(APART).await;
    let deleted = emulator.delete_and_settle(STREAM, "alpha").await;
    accepted.push(text(&deleted, "RequestToken"));

    let listed = list(&emulator, Some(100), None, &[], &[]).await;
    let mut expected = accepted.clone();
    expected.reverse();
    assert_eq!(
        reported(&listed),
        expected,
        "case every request: expected the newest first, {expected:?}, got {:?}",
        reported(&listed)
    );
    assert!(
        listed["NextToken"].is_null(),
        "case every request: the last page must carry no NextToken, got {listed:?}"
    );

    // Newest first means the reported `EventTime`s do not increase down the page.
    let times: Vec<f64> = summaries(&listed)
        .iter()
        .map(|summary| summary["EventTime"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        times.windows(2).all(|pair| pair[0] >= pair[1]),
        "case ordering: expected non-increasing EventTimes, got {times:?}"
    );

    // And each summary is the very `ProgressEvent` the status call reports for that
    // request: one listing and one status call cannot disagree about a request.
    for summary in summaries(&listed) {
        let token = text(&summary, "RequestToken");
        let (status, body) = emulator.status(&token).await;
        assert_eq!(
            status, 200,
            "case request {token:?}: expected HTTP 200 from GetResourceRequestStatus, got \
             {status} with body {body:?}"
        );
        assert_eq!(
            progress(&body),
            summary,
            "case request {token:?}: the listing and the status call must report the same \
             ProgressEvent, got {:?} and {summary:?}",
            progress(&body)
        );
        assert_eq!(
            text(&summary, "OperationStatus"),
            STATUS_SUCCESS,
            "case request {token:?}: expected a completed request, got {summary:?}"
        );
    }

    // Every operation type that ran is reported, each request exactly once.
    let mut operations: Vec<String> = summaries(&listed)
        .iter()
        .map(|summary| text(summary, "Operation"))
        .collect();
    operations.sort();
    assert_eq!(
        operations,
        vec![
            OPERATION_CREATE.to_string(),
            OPERATION_CREATE.to_string(),
            OPERATION_DELETE.to_string(),
            OPERATION_UPDATE.to_string(),
        ],
        "case operations: expected one request per operation that ran, got {operations:?}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_requests_the_filter_criteria_admit() {
    let emulator = Emulator::new();
    // Two creates, one update and one delete, all completed — so every record this case
    // filters over carries a known operation and a known status.
    let alpha = created(&emulator, STREAM, &json!({ "StreamName": "alpha" })).await;
    let bravo = created(&emulator, STREAM, &json!({ "StreamName": "bravo" })).await;
    let updated = emulator
        .update_and_settle(
            STREAM,
            "alpha",
            &json!([{ "op": "add", "path": "/Stage", "value": "prod" }]),
        )
        .await;
    let update = text(&updated, "RequestToken");
    let removed = emulator.delete_and_settle(STREAM, "bravo").await;
    let delete = text(&removed, "RequestToken");

    // The rows differ along what the criteria name: nothing, one operation, two
    // operations, one status, a status nothing reaches, and both criteria at once — the
    // last two of which narrow together rather than widening.
    let rows: [FilterRow; 8] = [
        ("no criterion at all", vec![], vec![], vec![&delete, &update, &bravo, &alpha]),
        (
            "one operation",
            vec![OPERATION_CREATE],
            vec![],
            vec![&bravo, &alpha],
        ),
        ("another operation", vec![OPERATION_DELETE], vec![], vec![&delete]),
        (
            "two operations",
            vec![OPERATION_UPDATE, OPERATION_DELETE],
            vec![],
            vec![&delete, &update],
        ),
        (
            "the status every request reached",
            vec![],
            vec![STATUS_SUCCESS],
            vec![&delete, &update, &bravo, &alpha],
        ),
        ("a status none reached", vec![], vec![STATUS_IN_PROGRESS], vec![]),
        (
            "a status the model declares and none reaches",
            vec![],
            vec![STATUS_CANCEL_COMPLETE],
            vec![],
        ),
        (
            "an operation and a status together",
            vec![OPERATION_UPDATE],
            vec![STATUS_SUCCESS],
            vec![&update],
        ),
    ];
    for (case, operations, statuses, expected) in rows {
        let listed = list(&emulator, Some(100), None, &operations, &statuses).await;
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            reported(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported(&listed)
        );
    }
}

#[tokio::test]
async fn a_request_is_listed_while_it_runs_and_in_the_status_it_has_reached() {
    let emulator = Emulator::new();
    // A request is answered while the operation it started is still running, so it is
    // listed in that status before it completes — and in the terminal one afterwards.
    let accepted = emulator
        .create_ok(STREAM, &json!({ "StreamName": "alpha" }), None)
        .await;
    let token = text(&accepted, "RequestToken");

    let running = list(&emulator, Some(100), None, &[], &[STATUS_IN_PROGRESS]).await;
    assert_eq!(
        reported(&running),
        vec![token.clone()],
        "case before completion: expected the running request, got {:?}",
        reported(&running)
    );
    let settled_filter = list(&emulator, Some(100), None, &[], &[STATUS_SUCCESS]).await;
    assert!(
        reported(&settled_filter).is_empty(),
        "case before completion: a completed-status filter must admit nothing yet, got {:?}",
        reported(&settled_filter)
    );

    emulator.settled(&token).await;

    let completed = list(&emulator, Some(100), None, &[], &[STATUS_SUCCESS]).await;
    assert_eq!(
        reported(&completed),
        vec![token.clone()],
        "case after completion: expected the completed request, got {:?}",
        reported(&completed)
    );
    assert_eq!(
        text(&summaries(&completed)[0], "Identifier"),
        "alpha",
        "case after completion: the summary must name the resource the request acted on, got \
         {completed:?}"
    );
    let still_running = list(&emulator, Some(100), None, &[], &[STATUS_IN_PROGRESS]).await;
    assert!(
        reported(&still_running).is_empty(),
        "case after completion: a running-status filter must admit nothing now, got {:?}",
        reported(&still_running)
    );
}

#[tokio::test]
async fn a_listing_pages_in_order_and_resumes_where_it_stopped() {
    let emulator = Emulator::new();
    // Seven requests of two operation types, so a page size that divides the set and one
    // that does not both have something to resume after — and so a narrowed walk has a
    // narrowed order to preserve.
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    for name in names {
        created(&emulator, STREAM, &json!({ "StreamName": name })).await;
        tokio::time::sleep(APART).await;
    }
    emulator
        .update_and_settle(
            STREAM,
            "alpha",
            &json!([{ "op": "add", "path": "/Stage", "value": "prod" }]),
        )
        .await;
    tokio::time::sleep(APART).await;
    emulator.delete_and_settle(STREAM, "echo").await;

    let whole = reported(&list(&emulator, Some(100), None, &[], &[]).await);
    assert_eq!(
        whole.len(),
        7,
        "case one page: expected every request, got {whole:?}"
    );

    // The rows differ along the page size: one per page, a size that divides the set, one
    // that does not, exactly the set, the modelled maximum, and no `MaxResults` at all.
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let walked = walk(&emulator, page_size, &[], &[]).await;
        assert_eq!(
            walked, whole,
            "page size {page_size:?}: expected {whole:?}, got {walked:?}"
        );
        let mut deduplicated = walked.clone();
        deduplicated.sort();
        deduplicated.dedup();
        assert_eq!(
            deduplicated.len(),
            walked.len(),
            "page size {page_size:?}: a walk must report no request twice, got {walked:?}"
        );
    }

    // A narrowed walk keeps that same order over its own, shorter set.
    let narrowed_whole = reported(&list(&emulator, Some(100), None, &[OPERATION_CREATE], &[]).await);
    for page_size in [Some(1), Some(2), Some(5)] {
        let walked = walk(&emulator, page_size, &[OPERATION_CREATE], &[]).await;
        assert_eq!(
            walked, narrowed_whole,
            "narrowed page size {page_size:?}: expected {narrowed_whole:?}, got {walked:?}"
        );
    }

    // A page that is not the last one carries a token; the last one does not.
    let first = list(&emulator, Some(3), None, &[], &[]).await;
    assert_eq!(
        reported(&first).len(),
        3,
        "case a page of three: expected three summaries, got {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case a page of three: a page with more to come must carry a NextToken, got {first:?}"
    );
    let resumed = list(&emulator, Some(3), Some(&token), &[], &[]).await;
    assert_eq!(
        reported(&resumed),
        whole[3..6].to_vec(),
        "case resumed: expected {:?}, got {:?}",
        whole[3..6].to_vec(),
        reported(&resumed)
    );
}

#[tokio::test]
async fn a_call_that_names_no_max_results_reports_the_documented_default_page() {
    let emulator = Emulator::new();
    // One request more than the documented default, so the default is observable as the
    // page it bounds rather than as "everything".
    let mut accepted = Vec::new();
    for index in 0..21 {
        let created = emulator
            .create_ok(STREAM, &json!({ "StreamName": format!("stream-{index:02}") }), None)
            .await;
        accepted.push(text(&created, "RequestToken"));
        tokio::time::sleep(APART).await;
    }
    for token in &accepted {
        emulator.settled(token).await;
    }

    let first = list(&emulator, None, None, &[], &[]).await;
    assert_eq!(
        reported(&first).len(),
        20,
        "case no MaxResults: expected the documented default of 20, got {} summaries",
        reported(&first).len()
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case no MaxResults: a page with more to come must carry a NextToken, got {first:?}"
    );
    let second = list(&emulator, None, Some(&token), &[], &[]).await;
    assert_eq!(
        reported(&second),
        vec![accepted[0].clone()],
        "case the second default page: expected the oldest request, got {:?}",
        reported(&second)
    );
    assert!(
        second["NextToken"].is_null(),
        "case the second default page: the last page must carry no NextToken, got {second:?}"
    );
}

#[tokio::test]
async fn a_filter_naming_a_value_outside_the_models_enum_is_refused() {
    let emulator = Emulator::new();
    // A list member's element type is stated about the LIST: the clause echoes the value
    // that member held — the WHOLE list the request sent, whichever of its elements failed
    // — names the list member, and nests the requirement every element had to satisfy. The
    // rows differ along which criterion carries the value, how many elements fail, and
    // whether the list also carries an element the enum admits.
    let rows: [EnumRow; 4] = [
        (
            "one status outside the enum",
            vec![],
            vec!["NOT_A_REAL_STATUS"],
            "[NOT_A_REAL_STATUS]",
            "resourceRequestStatusFilter.operationStatuses",
        ),
        (
            "two statuses outside the enum",
            vec![],
            vec!["NOT_A_REAL_STATUS", "ALSO_NOT_ONE"],
            "[NOT_A_REAL_STATUS, ALSO_NOT_ONE]",
            "resourceRequestStatusFilter.operationStatuses",
        ),
        (
            "a status the enum admits beside one it does not",
            vec![],
            vec!["SUCCESS", "NOT_A_REAL_STATUS"],
            "[SUCCESS, NOT_A_REAL_STATUS]",
            "resourceRequestStatusFilter.operationStatuses",
        ),
        (
            "one operation outside the enum",
            vec!["NOT_AN_OPERATION"],
            vec![],
            "[NOT_AN_OPERATION]",
            "resourceRequestStatusFilter.operations",
        ),
    ];
    for (case, operations, statuses, echoed, member) in rows {
        let (status, body) = emulator
            .cc(
                "ListResourceRequests",
                list_body(None, None, &operations, &statuses),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            code(&body)
        );
        let values = if member.ends_with("operations") {
            OPERATION_VALUES
        } else {
            OPERATION_STATUS_VALUES
        };
        assert_eq!(
            message(&body),
            format!(
                "1 validation error detected: Value '{echoed}' at '{member}' failed to \
                 satisfy constraint: Member must satisfy constraint: [Member must satisfy \
                 enum value set: [{values}]]"
            ),
            "case {case:?}: unexpected wording, got {:?}",
            message(&body)
        );
    }
}

#[tokio::test]
async fn a_continuation_token_this_listing_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    created(&emulator, STREAM, &json!({ "StreamName": "alpha" })).await;
    created(&emulator, STREAM, &json!({ "StreamName": "bravo" })).await;

    let minted = list(&emulator, Some(1), None, &[], &[]).await;
    let token = minted["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "the first of two pages must carry a NextToken, got {minted:?}"
    );
    let mut tampered = token.clone();
    tampered.pop();
    tampered.push(if token.ends_with('0') { '1' } else { '0' });
    // A token the RESOURCE listing minted: a value this store did mint, under the same
    // key, for another listing — so only the position inside it tells the two apart.
    let resource_listing = emulator.list(STREAM, Some(1), None).await;
    let resource_token = resource_listing["NextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !resource_token.is_empty(),
        "the resource listing must carry a NextToken, got {resource_listing:?}"
    );

    // The rows differ along how the value fails to be a token of this listing: not hex at
    // all, hex of the right shape, a minted token with one character changed, and a token
    // minted for the resource listing.
    let rows = [
        ("a value a caller wrote", "not-a-token".to_string()),
        ("hex of a minted token's shape", "00".repeat(40)),
        ("a minted token with one character changed", tampered),
        ("a token minted for the resource listing", resource_token),
    ];
    for (case, candidate) in rows {
        let (status, body) = emulator
            .cc(
                "ListResourceRequests",
                list_body(Some(1), Some(&candidate), &[], &[]),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert!(
            code(&body).contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            "Invalid NextToken",
            "case {case:?}: unexpected wording, got {:?}",
            message(&body)
        );
    }

    // And the page the token DOES name is still served, so the refusals above are about
    // the value and not about paging.
    let resumed = list(&emulator, Some(1), Some(&token), &[], &[]).await;
    assert_eq!(
        reported(&resumed).len(),
        1,
        "the minted token must resume this listing, got {resumed:?}"
    );
}
