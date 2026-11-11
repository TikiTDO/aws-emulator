// SPDX-License-Identifier: CC0-1.0

//! How `ListCustomActions` PAGES the custom actions of an account, driven over the wire
//! (restJson1) through the same `MultiServiceRouter` the production router serves with.
//!
//! The rules every assertion here is about:
//!
//! * **The page size is the modelled one.** `MaxResults` is `@range min 1 max 100`
//!   (`smithy_query(query="operation", operation="ListCustomActions")`), so a value outside
//!   that range is refused as this service's modelled client-input failure
//!   `InvalidRequestException` (HTTP 400) and nothing is reported, while each published
//!   bound is served.
//! * **A page never carries more than it was asked for, and a token is issued exactly when
//!   the scan has more to report.** Following the tokens visits every custom action the
//!   account holds exactly once, in one stable total order, whatever the page size; the
//!   last page of a scan carries no token, a caller that names no page size still gets the
//!   whole answer while the total is within the page a response is bounded to, and the
//!   token a page ends on is itself a value the modelled `NextToken` admits.
//! * **A listing reports and does not consume.** `ListCustomActions` is `@readonly`, so
//!   repeating a listing — the whole walk, or one page by its own token — answers exactly
//!   what it answered before, and every action it reported is still readable afterwards
//!   through `GetCustomAction`.
//!
//! The tables span what those rules generalise over: the value `MaxResults` carries, how
//! many actions the account holds (none, one, several), the page size a walk asks for
//! (every size from one to past the total, plus the published maximum and none at all),
//! and whether the store changed under a cursor a caller is holding.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Answer, Emulator};

/// The modelled client-input error class a constraint violation is refused with:
/// `InvalidRequestException` is the only one this model declares for bad input
/// (`smithy_query(query="operation", operation="ListCustomActions")`).
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The page a response is bounded to when the caller names no `MaxResults`.
const DEFAULT_PAGE_SIZE: i64 = chatbot::state::MAX_PAGE_SIZE as i64;

/// The published maximum of `MaxResults` (`@range min 1 max 100`).
const MAX_RESULTS_MAXIMUM: i64 = 100;

/// The published length maximum of the modelled `NextToken` (`length 1..1024`).
const NEXT_TOKEN_LENGTH_MAXIMUM: usize = 1024;

/// Create the custom action `action_name` and answer the ARN it was minted under.
async fn given_custom_action(emulator: &Emulator, action_name: &str) -> String {
    let created = emulator
        .call(
            "/create-custom-action",
            &json!({
                "ActionName": action_name,
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the custom action {action_name:?}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "creating {action_name:?}: no CustomActionArn in {}",
                created.body
            )
        })
        .to_string()
}

/// One `ListCustomActions` call: `max_results` as the page size, `next_token` as the cursor.
async fn list_custom_actions(
    emulator: &Emulator,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut request = json!({});
    if let Some(max) = max_results {
        request["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    emulator.call("/list-custom-actions", &request).await
}

/// The `CustomActions` list of an answer.
fn arns_of(body: &Value, case: &str) -> Vec<String> {
    body["CustomActions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CustomActions list in {body}"))
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a CustomActions entry is not a string: {arn}"))
                .to_string()
        })
        .collect()
}

/// Assert `token` is a value the modelled `NextToken` admits, so a client can carry it
/// back: `length 1..1024`, `pattern ^[\x20-\x7F]+$`.
fn assert_modelled_next_token(token: &str, case: &str) {
    assert!(
        !token.is_empty() && token.len() <= NEXT_TOKEN_LENGTH_MAXIMUM,
        "case {case:?}: a token must satisfy the modelled NextToken length 1..\
         {NEXT_TOKEN_LENGTH_MAXIMUM}, got {} characters",
        token.len()
    );
    assert!(
        token.chars().all(|c| ('\x20'..='\x7F').contains(&c)),
        "case {case:?}: a token must satisfy the modelled NextToken pattern of printable \
         ASCII, got {token:?}"
    );
}

/// Walk every page of the account's custom actions with `max_results` as the page size,
/// asserting the per-page invariants, and answer the pages in order.
async fn walk_pages(emulator: &Emulator, max_results: Option<i64>, case: &str) -> Vec<Vec<String>> {
    let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let answer = list_custom_actions(emulator, max_results, next_token.as_deref()).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} page {}: expected HTTP 200, got {} with {}",
            pages.len(),
            answer.status,
            answer.body
        );
        let page = arns_of(&answer.body, case);
        assert!(
            page.len() as i64 <= page_size,
            "case {case:?} page {}: a page carries at most {page_size} action(s), got {}",
            pages.len(),
            page.len()
        );
        let token = answer.body["NextToken"].as_str().map(str::to_string);
        if let Some(token) = &token {
            assert_modelled_next_token(token, case);
            // A token means the scan has more to report, which it only can once the page it
            // just filled was as large as the caller allowed.
            assert_eq!(
                page.len() as i64,
                page_size,
                "case {case:?} page {}: a token is issued only on a full page, got {} \
                 action(s) in {}",
                pages.len(),
                page.len(),
                answer.body
            );
        }
        pages.push(page);
        match token {
            None => return pages,
            Some(token) => next_token = Some(token),
        }
        assert!(
            pages.len() < 32,
            "case {case:?}: the walk issued {} pages without ending",
            pages.len()
        );
    }
}

/// The ARNs of every page of a walk, concatenated.
fn flattened(pages: &[Vec<String>]) -> Vec<String> {
    pages.iter().flatten().cloned().collect()
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused_and_each_bound_is_served() {
    let emulator = Emulator::new();
    let arn = given_custom_action(&emulator, "bounds-probe").await;

    // `MaxResults` is `@range min 1 max 100`: each row is a value outside it — both
    // published bounds crossed by one, and further out in each direction.
    let refused: [(&str, i64); 5] = [
        ("one below the range minimum", 0),
        ("negative", -1),
        ("far below the range minimum", -100),
        ("one above the range maximum", MAX_RESULTS_MAXIMUM + 1),
        ("far above the range maximum", 100_000),
    ];
    for (case, max_results) in refused {
        let answer = list_custom_actions(&emulator, Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (MaxResults={max_results}): expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {case:?} (MaxResults={max_results}): expected the modelled \
             {INVALID_REQUEST_EXCEPTION}, got {:?} in {}",
            answer.code(),
            answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_REQUEST_EXCEPTION),
            "case {case:?} (MaxResults={max_results}): expected x-amzn-errortype \
             {INVALID_REQUEST_EXCEPTION}, got {:?}",
            answer.error_type
        );
    }

    // Each row is a value INSIDE the range — the two published bounds and one between them
    // — and is served the one action the account holds, with no token, since nothing is
    // left over.
    let served: [(&str, Option<i64>); 4] = [
        ("the range minimum", Some(1)),
        ("between the bounds", Some(7)),
        ("the range maximum", Some(MAX_RESULTS_MAXIMUM)),
        ("no page size at all", None),
    ];
    for (case, max_results) in served {
        let answer = list_custom_actions(&emulator, max_results, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            arns_of(&answer.body, case),
            vec![arn.clone()],
            "case {case:?}: expected the one action the account holds, got {}",
            answer.body
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case:?}: a page that exhausted the scan carries no token, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn every_page_size_reports_the_same_actions_exactly_once_and_ends_without_a_token() {
    let emulator = Emulator::new();

    // An account holding nothing is answered one empty page at any page size, rather than
    // a refusal.
    for max_results in [Some(1), Some(MAX_RESULTS_MAXIMUM), None] {
        let case = format!("no action stored, MaxResults={max_results:?}");
        let pages = walk_pages(&emulator, max_results, &case).await;
        assert_eq!(
            pages,
            vec![Vec::<String>::new()],
            "case {case:?}: expected one empty page, got {pages:?}"
        );
    }

    // Each row holds one more action than the row before, so the claim below is made over a
    // scan that fits in a page, one that is exactly a page and ones that need several — and
    // the names differ in length and charset, so what is reported cannot follow from the
    // spelling of one ARN.
    let action_names = ["a", "paged-action", "Mixed_Case-9", &"z".repeat(64), "b-2"];
    let mut expected: Vec<String> = Vec::new();
    for action_name in action_names {
        expected.push(given_custom_action(&emulator, action_name).await);
        expected.sort();

        // Every page size from one action to past the total, plus the published maximum and
        // no page size at all: the SAME total answer, split differently.
        let page_sizes: Vec<Option<i64>> = (1..=(expected.len() as i64 + 2))
            .map(Some)
            .chain([Some(MAX_RESULTS_MAXIMUM), None])
            .collect();
        for max_results in page_sizes {
            let case = format!("{} stored, MaxResults={max_results:?}", expected.len());
            let pages = walk_pages(&emulator, max_results, &case).await;
            assert_eq!(
                flattened(&pages),
                expected,
                "case {case:?}: expected every action exactly once in one stable order, got {} \
                 page(s): {pages:?}",
                pages.len()
            );
            // The walk ends because the last page carried no token, so it is the only page
            // that may be short — and a scan is never split into more pages than its total
            // needs: a token is issued only while something is left over, so a total that
            // divides evenly into the page size needs no extra, empty page.
            let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
            let total = expected.len() as i64;
            let expected_pages = if total == 0 {
                1
            } else {
                total.div_euclid(page_size) + i64::from(total % page_size != 0)
            };
            assert_eq!(
                pages.len() as i64,
                expected_pages,
                "case {case:?}: expected {expected_pages} page(s), got {}: {pages:?}",
                pages.len()
            );
        }
    }
}

#[tokio::test]
async fn a_listing_reports_without_consuming_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let mut expected = Vec::new();
    for action_name in ["read-1", "read-2", "read-3"] {
        expected.push(given_custom_action(&emulator, action_name).await);
    }
    expected.sort();

    // The first page of a two-at-a-time walk, and the token it ends on.
    let first = list_custom_actions(&emulator, Some(2), None).await;
    assert_eq!(
        first.status, 200,
        "the first page is served, got {} with {}",
        first.status, first.body
    );
    let token = first.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "a scan with more to report issues a token, got {}",
                first.body
            )
        })
        .to_string();

    // Reading the same position again — the first page, and the page the token names —
    // answers exactly what it answered before, however many times it is asked for.
    for attempt in 0..3 {
        let repeated = list_custom_actions(&emulator, Some(2), None).await;
        assert_eq!(
            arns_of(&repeated.body, "the first page, repeated"),
            arns_of(&first.body, "the first page"),
            "attempt {attempt}: re-reading the first page must answer the same actions, got {}",
            repeated.body
        );
        assert_eq!(
            repeated.body["NextToken"].as_str(),
            Some(token.as_str()),
            "attempt {attempt}: re-reading the first page must answer the same token, got {}",
            repeated.body
        );

        let resumed = list_custom_actions(&emulator, Some(2), Some(&token)).await;
        assert_eq!(
            resumed.status, 200,
            "attempt {attempt}: resuming from the token is served, got {} with {}",
            resumed.status, resumed.body
        );
        assert_eq!(
            arns_of(&resumed.body, "the page the token names"),
            expected[2..].to_vec(),
            "attempt {attempt}: expected the actions after the cursor, got {}",
            resumed.body
        );
    }

    // And every action a read reported is still there afterwards, readable one by one:
    // a listing reported them and consumed none.
    for arn in &expected {
        let read_back = emulator
            .call("/get-custom-action", &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            read_back.status, 200,
            "an action a listing reported must still be readable: {arn} answered {} with {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.body["CustomAction"]["CustomActionArn"].as_str(),
            Some(arn.as_str()),
            "reading {arn} back must report that same action, got {}",
            read_back.body
        );
    }
    let after = walk_pages(&emulator, None, "after the repeated reads").await;
    assert_eq!(
        flattened(&after),
        expected,
        "the actions a read reported must still be listed, got {after:?}"
    );
}

#[tokio::test]
async fn a_cursor_survives_the_store_changing_under_it() {
    let emulator = Emulator::new();
    // Five actions, so a page of two leaves a cursor with several actions behind it.
    let mut stored = Vec::new();
    for action_name in ["c1", "c2", "c3", "c4", "c5"] {
        stored.push(given_custom_action(&emulator, action_name).await);
    }
    stored.sort();

    let first = list_custom_actions(&emulator, Some(2), None).await;
    assert_eq!(
        first.status, 200,
        "the first page is served, got {} with {}",
        first.status, first.body
    );
    let first_page = arns_of(&first.body, "the first page of a two-at-a-time walk");
    assert_eq!(
        first_page.len(),
        2,
        "a page of two over five stored actions carries two, got {first_page:?}"
    );
    // The position the cursor names: the action the first page ended on.
    let cursor_arn = first_page[1].clone();
    let cursor = first.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a scan with more to report issues a token, got {}", first.body))
        .to_string();

    // Each row changes the store between the two pages, differing in WHERE the change
    // falls relative to the cursor: behind it (already reported), ahead of it (not yet
    // reported), and the action the cursor itself names. A cursor is a position in the
    // scan, so the second page reports what is still stored strictly after it — never a
    // deleted action, and never one it already reported.
    let rows: [(&str, &str); 3] = [
        ("an action behind the cursor is deleted", "c1"),
        ("the action the cursor names is deleted", "c2"),
        ("an action ahead of the cursor is deleted", "c4"),
    ];
    let mut remaining = stored.clone();
    for (case, deleted_name) in rows {
        let deleted_arn = remaining
            .iter()
            .find(|arn| arn.ends_with(&format!("/{deleted_name}")))
            .unwrap_or_else(|| panic!("case {case:?}: {deleted_name} is no longer stored"))
            .clone();
        let deleted = emulator
            .call(
                "/delete-custom-action",
                &json!({"CustomActionArn": deleted_arn}),
            )
            .await;
        assert!(
            deleted.status == 200 || deleted.status == 204,
            "case {case:?}: the delete is served, got {} with {}",
            deleted.status,
            deleted.body
        );
        remaining.retain(|arn| *arn != deleted_arn);

        let resumed = list_custom_actions(&emulator, Some(MAX_RESULTS_MAXIMUM), Some(&cursor)).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: resuming from the cursor is served, got {} with {}",
            resumed.status, resumed.body
        );
        let expected_after_cursor: Vec<String> = remaining
            .iter()
            .filter(|arn| arn.as_str() > cursor_arn.as_str())
            .cloned()
            .collect();
        assert_eq!(
            arns_of(&resumed.body, case),
            expected_after_cursor,
            "case {case:?}: expected what is still stored after the cursor, got {}",
            resumed.body
        );
    }
}
