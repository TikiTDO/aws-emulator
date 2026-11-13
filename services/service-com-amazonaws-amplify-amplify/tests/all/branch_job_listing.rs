// SPDX-License-Identifier: CC0-1.0

//! `ListJobs`: one branch's job history, reported and paged out of persisted state.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `ListJobs` reports a page of the jobs the branch it names HOLDS, each summary
//!    being the record `GetJob` answers for that job, NEWEST JOB FIRST, with `nextToken`
//!    the id of the first job the page did not return and absent on the last page. A
//!    branch that has run nothing lists nothing.
//! 2. The page bound is `maxResults`: a page holds `min(bound, remaining)` jobs, a
//!    caller that set no bound (member absent, or its modelled default of 0) gets the
//!    modelled maximum, and a bound past the modelled range maximum is refused rather
//!    than served.
//! 3. A `nextToken` this service never issued is refused with the modelled
//!    `BadRequestException`; every token it does issue is a `jobId`, and one naming an
//!    id the branch no longer holds resumes at the next id.
//! 4. The collection is addressed through its branch and its app, so an unknown app or
//!    an unknown branch is the modelled 404 and no other branch's job can appear.
//! 5. Listing is a report: it materialises only the status change the clock had already
//!    made due, so two listings of a settled history are identical and agree with
//!    `GetJob`.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::JOB_SETTLE_MILLIS;

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// The modelled `range` maximum of `ListJobs$maxResults`, which is also the page a
/// caller that sets no bound receives
/// (`smithy_query(query="operation",operation="ListJobs")`).
const MODELLED_MAX_RESULTS: usize = 50;

/// `jobType` values used to start the jobs under test, from the model
/// (`smithy_query(query="shape",shape="JobType")`: `RELEASE | RETRY | MANUAL |
/// WEB_HOOK`). `RETRY` is excluded on purpose: it re-runs an existing job instead of
/// adding one to the history.
const STARTING_JOB_TYPES: &[&str] = &["RELEASE", "MANUAL", "WEB_HOOK"];

/// The status a job carries from `StartJob` until the clock settles it
/// (`smithy_query(query="shape",shape="JobStatus")`).
const PENDING: &str = "PENDING";

/// The terminal status a job of this emulator settles to: no build environment exists
/// here, so no build ran.
const FAILED: &str = "FAILED";

/// How long a test waits before expecting the clock-owed status change: the span the
/// service itself publishes, plus a margin for a slow machine.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 250)
}

/// An app with one branch on it, and the branch's ARN (which every `jobArn` extends).
///
/// The app is CONNECTED TO A REPOSITORY, because every case here starts jobs to list:
/// `StartJob` runs a build of the app's repository, so an app with no repository
/// provider refuses it (see `start_job_repository_precondition.rs`).
async fn app_with_branch(emulator: &Emulator, name: &str, branch: &str) -> (String, String) {
    let app_id = emulator.app_id_of_new_repository_app(name).await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let branch_arn = string_member(&created.member("branch"), "branchArn");
    (app_id, branch_arn)
}

/// `StartJob` over the wire.
async fn start_job(emulator: &Emulator, app_id: &str, branch: &str, job_type: &str) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/jobs"),
            Some(&json!({ "jobType": job_type })),
        )
        .await
}

/// `GetJob` over the wire.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
}

/// `ListJobs` over the wire, with `query` appended verbatim (`""` for no query).
async fn list_jobs(emulator: &Emulator, app_id: &str, branch: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs{query}"),
            None,
        )
        .await
}

/// Start `count` jobs on `branch`, cycling the `jobType`s, and answer the ids the
/// branch minted for them in the order a listing reports them.
///
/// A branch mints decimal ids counting its own jobs and reports its history newest job
/// first, so the expected order is the ids reversed — which once a branch has run more
/// than nine jobs is NOT their text order either way, and so is worth spanning.
async fn history_of(emulator: &Emulator, app_id: &str, branch: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let job_type = STARTING_JOB_TYPES[index % STARTING_JOB_TYPES.len()];
        let started = start_job(emulator, app_id, branch, job_type).await;
        assert_eq!(
            started.status, 200,
            "case \"history of {count}\": expected HTTP 200 from StartJob, got {started:?}"
        );
        ids.push(string_member(&started.member("jobSummary"), "jobId"));
    }
    ids.reverse();
    ids
}

/// The `jobId`s a listing reply reports, in the order it reported them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    list_member(&reply.body, "jobSummaries")
        .iter()
        .map(|summary| string_member(summary, "jobId"))
        .collect()
}

/// The `nextToken` a listing reply carries, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn list_jobs_answers_the_jobs_the_branch_holds_for_every_history_size() {
    let emulator = Emulator::new();

    // The sizes span a branch that has run nothing, one job, a handful, and enough
    // jobs that the ids are no longer single digits (so the reported order is the
    // branch's own job order rather than an accident of one-character ids).
    for count in [0_usize, 1, 5, 12] {
        let branch = "main";
        let (app_id, branch_arn) =
            app_with_branch(&emulator, &format!("listing-{count}"), branch).await;
        let expected = history_of(&emulator, &app_id, branch, count).await;

        let reply = list_jobs(&emulator, &app_id, branch, "").await;
        assert_eq!(
            reply.status, 200,
            "case \"{count} jobs\": expected HTTP 200 from ListJobs, got {reply:?}"
        );
        assert_eq!(
            listed_ids(&reply),
            expected,
            "case \"{count} jobs\": ListJobs must report the branch's whole history \
             newest job first, got {:?}",
            reply.body
        );
        assert!(
            next_token(&reply).is_none(),
            "case \"{count} jobs\": a page that reached the end carries no nextToken, \
             got {:?}",
            next_token(&reply)
        );

        // Every listed summary is the record the reader of that one job answers, so a
        // listing and a `GetJob` cannot drift.
        for summary in list_member(&reply.body, "jobSummaries") {
            let job_id = string_member(summary, "jobId");
            assert_eq!(
                string_member(summary, "jobArn"),
                format!("{branch_arn}/jobs/{job_id:0>10}"),
                "case \"{count} jobs\": a listed job is addressed inside its branch, with \
                 the job number in the ten-digit field every job ARN carries, got {summary:?}"
            );
            let read = get_job(&emulator, &app_id, branch, &job_id).await;
            assert_eq!(
                read.status, 200,
                "case \"{count} jobs\": expected HTTP 200 from GetJob {job_id}, got {read:?}"
            );
            assert_eq!(
                read.member("job")
                    .get("summary")
                    .cloned()
                    .unwrap_or(Value::Null),
                *summary,
                "case \"{count} jobs\": job {job_id} must be listed as GetJob answers it, \
                 got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn list_jobs_pages_over_the_whole_history_for_every_page_bound() {
    let emulator = Emulator::new();
    let branch = "release";
    let (app_id, _) = app_with_branch(&emulator, "paging", branch).await;
    let expected: Vec<String> = history_of(&emulator, &app_id, branch, 12).await;
    let expected_set: BTreeSet<String> = expected.iter().cloned().collect();

    // The bounds span 1, a middle value, exactly the number of records, more than the
    // number of records, the modelled `range` maximum, and the two "caller set no
    // bound" cases: the member absent, and its modelled default of 0.
    let bounds: Vec<(&str, String, Option<usize>)> = vec![
        ("maxResults=1", "?maxResults=1".to_string(), Some(1)),
        ("maxResults=5", "?maxResults=5".to_string(), Some(5)),
        ("maxResults=12", "?maxResults=12".to_string(), Some(12)),
        ("maxResults=13", "?maxResults=13".to_string(), Some(13)),
        ("maxResults=50", "?maxResults=50".to_string(), Some(50)),
        ("maxResults absent", String::new(), None),
        ("maxResults=0", "?maxResults=0".to_string(), None),
    ];

    for (label, query, bound) in bounds {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let paged = match &token {
                Some(value) => format!("{query}{separator}nextToken={value}"),
                None => query.clone(),
            };
            let reply = list_jobs(&emulator, &app_id, branch, &paged).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from ListJobs, got {reply:?}"
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages"
            );

            let page = listed_ids(&reply);
            let remaining = expected.len() - seen.len();
            let want = bound.unwrap_or(MODELLED_MAX_RESULTS).min(remaining);
            assert_eq!(
                page.len(),
                want,
                "case {label:?}: page {pages} must hold min(bound, remaining) = {want} \
                 jobs, got {page:?}"
            );
            seen.extend(page);

            token = next_token(&reply);
            match &token {
                // The token is the id of the first job the page did NOT return.
                Some(value) => assert_eq!(
                    Some(value),
                    expected.get(seen.len()),
                    "case {label:?}: the nextToken must name the first job not returned, \
                     got {value:?}"
                ),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: a paged walk must visit the whole history once, in order, \
             got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected_set,
            "case {label:?}: a paged walk must visit every job of the branch"
        );
    }

    // One past the modelled maximum is refused by the boundary validator, not served.
    let refused = list_jobs(&emulator, &app_id, branch, "?maxResults=51").await;
    assert_eq!(
        refused.status, 400,
        "case \"maxResults=51\": a value past the modelled range maximum must be \
         refused, got {refused:?}"
    );
}

#[tokio::test]
async fn list_jobs_refuses_a_token_it_never_issued_and_resumes_at_one_it_did() {
    let emulator = Emulator::new();
    let branch = "main";
    let (app_id, _) = app_with_branch(&emulator, "tokens", branch).await;
    let expected = history_of(&emulator, &app_id, branch, 4).await;

    // Every token this operation issues is a `jobId` (modelled `pattern ^[0-9]+$`), so
    // a value that could not be one was never issued: it is refused rather than served
    // as an arbitrary suffix of the history.
    for (label, token) in [
        ("token that is not a job id", "not-a-job-id"),
        ("token with a trailing letter", "1a"),
        ("negative token", "-1"),
        ("hexadecimal token", "0x10"),
        ("token with a non-ASCII digit", "٣"),
    ] {
        let reply = list_jobs(&emulator, &app_id, branch, &format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: an unissued nextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            reply.error_code
        );
    }

    // A token the service could have issued resumes the scan at the first id at or
    // after it IN THE REPORTED ORDER, which runs downwards from the newest job —
    // including an id the branch does not hold, which is how a page boundary survives a
    // history that shrank between two calls.
    for (label, token, want) in [
        ("token naming the newest job", "4", 0),
        ("token naming a job in the middle", "3", 1),
        ("token naming the oldest job", "1", 3),
        ("token naming no job, above the whole history", "25", 0),
        ("token naming no job, below the whole history", "0", 4),
    ] {
        let reply = list_jobs(&emulator, &app_id, branch, &format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected HTTP 200 from ListJobs, got {reply:?}"
        );
        assert_eq!(
            listed_ids(&reply),
            expected[want..].to_vec(),
            "case {label:?}: a resumed scan starts at the first id at or after the \
             token, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn list_jobs_reports_only_the_named_branch_of_the_named_app() {
    let emulator = Emulator::new();

    // Two apps, each with two branches, each branch with its own number of jobs: a
    // listing may report only the branch it names.
    let mut histories: Vec<(String, &str, Vec<String>)> = Vec::new();
    for (app_name, branches) in [
        ("scoped-a", [("main", 3_usize), ("dev", 1)]),
        ("scoped-b", [("main", 2), ("feature-x", 0)]),
    ] {
        let app_id = emulator.app_id_of_new_repository_app(app_name).await;
        for (branch, count) in branches {
            let created = emulator
                .create_branch(&app_id, &json!({ "branchName": branch }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {app_name:?}/{branch:?}: expected HTTP 200 from CreateBranch, \
                 got {created:?}"
            );
            let ids = history_of(&emulator, &app_id, branch, count).await;
            histories.push((app_id.clone(), branch, ids));
        }
    }

    for (app_id, branch, ids) in &histories {
        let reply = list_jobs(&emulator, app_id, branch, "").await;
        assert_eq!(
            reply.status, 200,
            "case {branch:?}: expected HTTP 200 from ListJobs, got {reply:?}"
        );
        assert_eq!(
            &listed_ids(&reply),
            ids,
            "case {branch:?}: a listing reports that branch's jobs and no other's, \
             got {:?}",
            reply.body
        );
    }

    // An app or a branch the store does not hold is the modelled 404, naming the
    // entity that is actually absent.
    let (app_id, _, _) = &histories[0];
    for (label, listed_app, listed_branch, entity, identifier) in [
        ("unknown app", "d000unknownapp", "main", "App", "d000unknownapp"),
        ("unknown branch", app_id.as_str(), "nope", "Branch", "nope"),
    ] {
        let reply = list_jobs(&emulator, listed_app, listed_branch, "").await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message(),
            Some(format!("{entity} {identifier} not found.")),
            "case {label:?}: the 404 must name the absent entity, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn list_jobs_reports_the_status_the_clock_owes_and_changes_nothing_else() {
    let emulator = Emulator::new();
    let branch = "main";
    let (app_id, _) = app_with_branch(&emulator, "settling", branch).await;
    let expected = history_of(&emulator, &app_id, branch, 3).await;

    // The settle span is non-zero, so a listing taken straight after the starts still
    // reports the non-terminal status a real client has to poll through.
    let pending = list_jobs(&emulator, &app_id, branch, "").await;
    for summary in list_member(&pending.body, "jobSummaries") {
        assert_eq!(
            string_member(summary, "status"),
            PENDING,
            "case \"before the span\": a listed job that has not settled is \
             non-terminal, got {summary:?}"
        );
    }

    sleep(past_the_settle_span());

    let settled = list_jobs(&emulator, &app_id, branch, "").await;
    assert_eq!(
        listed_ids(&settled),
        expected,
        "case \"after the span\": settling changes a job's status, not the history, \
         got {:?}",
        settled.body
    );
    for summary in list_member(&settled.body, "jobSummaries") {
        let job_id = string_member(summary, "jobId");
        assert_eq!(
            string_member(summary, "status"),
            FAILED,
            "case \"after the span\": the clock owes job {job_id} a terminal status, \
             got {summary:?}"
        );
        // The listing and the single-job reader answer the same persisted record.
        let read = get_job(&emulator, &app_id, branch, &job_id).await;
        assert_eq!(
            read.member("job")
                .get("summary")
                .cloned()
                .unwrap_or(Value::Null),
            *summary,
            "case \"after the span\": job {job_id} must be listed as GetJob answers it, \
             got {:?}",
            read.body
        );
    }

    // A read changes nothing a later caller sees: the second listing is identical.
    let again = list_jobs(&emulator, &app_id, branch, "").await;
    assert_eq!(
        again.body, settled.body,
        "case \"listed twice\": a report must not change what a later caller sees, \
         got {:?} then {:?}",
        settled.body, again.body
    );
}
