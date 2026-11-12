// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that `MaxResults` is the ONLY page bound
//! `ListSpeechSynthesisTasks` has: a request that names it gets at most that many tasks
//! and a `NextToken` while a further matching task exists, and a request that names no
//! bound gets every matching task in one response with no `NextToken`.
//!
//! The rule is checked over a TABLE of store sizes rather than the one size an issue
//! happens to quote: empty, one, several sizes either side of any page a reader might
//! expect, and stores larger than the largest `MaxResults` the model allows — so a
//! hidden cap anywhere in the range could not pass. The second table spans the
//! `MaxResults` values themselves, including both published bounds and a bound at,
//! below and above the store size.
//!
//! The handler's own ceiling on how much ONE response materialises
//! (`REPORT_MATERIALISATION_CEILING`) is the one bound no request can name, so it is
//! checked on its own: a store past it pages through the cursor rather than dropping
//! anything.
//!
//! The harness and the JSON readers come from `crate::lexicons_and_speech`, the page
//! readers from `crate::synthesis_tasks` and the task fixtures from `crate::support`:
//! a store of a size and a creation order a row fixes is seeded directly, the way
//! `StartSpeechSynthesisTask` writes a task.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_polly_parrot_v1::ops::list_speech_synthesis_tasks::REPORT_MATERIALISATION_CEILING;

use crate::lexicons_and_speech::{json, Polly};
use crate::support::{fixture_task_id, seed_task, stored_task};
use crate::synthesis_tasks::{list, task_ids};

/// Store sizes the rule is checked over. 0 and 1 are the degenerate stores, 19/20/21
/// bracket the page a reader might expect of a list operation, and 100 and 137 are at
/// and beyond `MaxResults`'s published maximum, so a store too large for any single
/// bounded page is covered too.
const STORE_SIZES: [usize; 8] = [0, 1, 2, 19, 20, 21, 100, 137];

/// The published bounds of `MaxResults`, from
/// `smithy_query(query="operation", operation="ListSpeechSynthesisTasks")`: an integer
/// with `range {min: 1, max: 100}`.
const MAX_RESULTS_BOUNDS: (usize, usize) = (1, 100);

/// The id of the `index`th seeded task, in the minted form Amazon Polly uses (a
/// canonical UUID), so the task is readable through `GetSpeechSynthesisTask` too. The
/// report's order comes from the creation dates below, not from the id.
fn task_name(prefix: &str, index: usize) -> String {
    fixture_task_id(&format!("{prefix}-{index}"))
}

/// Seed `count` tasks in `status`, oldest first, and return their ids in the order the
/// report must use — the reverse, because the report is newest first.
///
/// The creation dates are ten milliseconds apart, so every task has a distinct
/// millisecond order key and the expected order is unambiguous.
fn seed_tasks(polly: &Polly, prefix: &str, count: usize, status: &str) -> Vec<String> {
    let base = SystemTime::now() - Duration::from_secs(3_600);
    let mut seeded: Vec<String> = (0..count)
        .map(|index| {
            let task_id = task_name(prefix, index);
            let offset_millis = u64::try_from(index).expect("a seeded index fits a u64") * 10;
            let mut task = stored_task(&task_id, base + Duration::from_millis(offset_millis));
            task.task_status = status.to_string();
            seed_task(polly, &task);
            task_id
        })
        .collect();
    seeded.reverse();
    seeded
}

/// RULE: a request that names no `MaxResults` is not paged — it reports every matching
/// task in one response, newest first, with no `NextToken`.
#[tokio::test]
async fn an_unbounded_request_reports_every_task_in_one_response() {
    for size in STORE_SIZES {
        let case = format!("{size} stored tasks, no MaxResults");
        let polly = Polly::new();
        let expected = seed_tasks(&polly, "Task", size, "completed");

        let response = list(&polly, "").await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: ListSpeechSynthesisTasks failed, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        let body = json(&response);
        let reported = task_ids(&body);
        assert_eq!(
            reported,
            expected,
            "case {case:?}: one response must carry every task, newest first, got {} tasks",
            reported.len()
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "case {case:?}: nothing was left over, so no NextToken is owed, got {:?}",
            body["NextToken"]
        );

        // The report must not contradict the per-task read: the task a truncated
        // report would have dropped is both reported above and readable here.
        if let Some(last) = expected.last() {
            let got = polly
                .call("GET", &format!("/v1/synthesisTasks/{last}"), "")
                .await;
            assert_eq!(
                got.status(),
                200,
                "case {case:?}: GetSpeechSynthesisTask {last} must agree with the report, \
                 body {:?}",
                String::from_utf8_lossy(got.body())
            );
        }
    }
}

/// RULE: `MaxResults` is the only bound that truncates — a page carries
/// `min(MaxResults, remaining)` tasks and a `NextToken` appears exactly while a further
/// matching task exists.
#[tokio::test]
async fn max_results_is_the_only_bound_that_truncates() {
    let size = 25usize;
    let polly = Polly::new();
    let expected = seed_tasks(&polly, "Task", size, "completed");

    let (min_bound, max_bound) = MAX_RESULTS_BOUNDS;
    // Both published bounds, sizes in between, and a bound at, just under and just
    // over the store size.
    for max_results in [min_bound, 2, 7, size - 1, size, size + 1, max_bound] {
        let case = format!("MaxResults={max_results} over {size} tasks");
        let body = json(&list(&polly, &format!("?MaxResults={max_results}")).await);
        let reported = task_ids(&body);
        let expected_page = &expected[..max_results.min(size)];
        assert_eq!(
            reported, expected_page,
            "case {case:?}: expected the first {} tasks, got {reported:?}",
            expected_page.len()
        );
        assert_eq!(
            body["NextToken"].as_str().is_some(),
            size > max_results,
            "case {case:?}: a NextToken is owed exactly while a further task exists, got {:?}",
            body["NextToken"]
        );
    }

    // A token minted under a bound, resumed WITHOUT one: the remainder, whole, with no
    // token of its own — the bound travels with the request, not with the cursor.
    for first_page in [min_bound, 7, size - 1] {
        let case = format!("resume after a {first_page}-task page without MaxResults");
        let token = json(&list(&polly, &format!("?MaxResults={first_page}")).await)["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the first page must leave a NextToken"))
            .to_string();
        let body = json(&list(&polly, &format!("?NextToken={token}")).await);
        let reported = task_ids(&body);
        assert_eq!(
            reported,
            &expected[first_page..],
            "case {case:?}: the remainder must be reported whole, got {} tasks",
            reported.len()
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "case {case:?}: the remainder fits, so no NextToken is owed, got {:?}",
            body["NextToken"]
        );
    }
}

/// RULE: the one bound the caller cannot name — the ceiling on how much a single
/// response materialises (`REPORT_MATERIALISATION_CEILING`, the handler's own) — pages
/// the report rather than dropping any of it: a store past the ceiling answers exactly
/// the ceiling's worth of tasks plus a `NextToken`, and following it reports the rest.
#[tokio::test]
async fn a_store_past_the_materialisation_ceiling_pages_rather_than_drops() {
    let over = 3usize;
    let size = REPORT_MATERIALISATION_CEILING + over;
    let polly = Polly::new();
    let expected = seed_tasks(&polly, "Task", size, "completed");

    let body = json(&list(&polly, "").await);
    let first = task_ids(&body);
    assert_eq!(
        first.len(),
        REPORT_MATERIALISATION_CEILING,
        "case {size} stored tasks: one response materialises at most \
         {REPORT_MATERIALISATION_CEILING} tasks, got {}",
        first.len()
    );
    assert_eq!(
        first,
        expected[..REPORT_MATERIALISATION_CEILING],
        "case {size} stored tasks: the first response must be the newest tasks, in order"
    );
    let token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {size} stored tasks: tasks were left over, so a NextToken is owed, got {:?}", body["NextToken"]))
        .to_string();

    let rest = json(&list(&polly, &format!("?NextToken={token}")).await);
    assert_eq!(
        task_ids(&rest),
        expected[REPORT_MATERIALISATION_CEILING..],
        "case {size} stored tasks: the remaining {over} tasks must be reported, not dropped"
    );
    assert_eq!(
        rest["NextToken"].as_str(),
        None,
        "case {size} stored tasks: the remainder fits, so no NextToken is owed, got {:?}",
        rest["NextToken"]
    );
}

/// RULE: the same bound rule holds under a `Status` filter — an unbounded request
/// reports every task in that status, however many there are, and `MaxResults` bounds
/// the filtered report and nothing else.
#[tokio::test]
async fn the_bound_rule_holds_under_a_status_filter() {
    let polly = Polly::new();
    // Two statuses whose counts both exceed any page a reader might expect of a list
    // operation, so a filtered report that is truncated could not pass.
    let completed = seed_tasks(&polly, "Done", 23, "completed");
    let failed = seed_tasks(&polly, "Fail", 31, "failed");

    for (status, expected) in [("completed", &completed), ("failed", &failed)] {
        let case = format!("Status={status} with {} tasks, no MaxResults", expected.len());
        let body = json(&list(&polly, &format!("?Status={status}")).await);
        let reported = task_ids(&body);
        assert_eq!(
            reported,
            *expected,
            "case {case:?}: every task in that status must be reported, got {} tasks",
            reported.len()
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "case {case:?}: nothing was left over, so no NextToken is owed, got {:?}",
            body["NextToken"]
        );

        // The same filter, bounded: now the caller's bound is what truncates it.
        let bounded = json(&list(&polly, &format!("?Status={status}&MaxResults=5")).await);
        assert_eq!(
            task_ids(&bounded),
            expected[..5],
            "case {case:?}: MaxResults=5 must carry the first five of the filtered report"
        );
        assert!(
            bounded["NextToken"].as_str().is_some(),
            "case {case:?}: MaxResults=5 leaves further tasks, so a NextToken is owed"
        );
    }

    // Unfiltered, unbounded: both statuses together, still in one response.
    let body = json(&list(&polly, "").await);
    assert_eq!(
        task_ids(&body).len(),
        completed.len() + failed.len(),
        "an unfiltered unbounded report must carry every stored task"
    );
    assert_eq!(
        body["NextToken"].as_str(),
        None,
        "an unfiltered unbounded report is not truncated, got {:?}",
        body["NextToken"]
    );
}
