// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListWebFunctionRevisions`.
//!
//! RULE: a listing reports one page of the revisions THIS web function holds, in one
//! stable order, narrowed by the documented `state` filter, and hands out a `nextToken`
//! exactly while a matching revision remains — so following the token walks every
//! matching revision exactly once, whatever the page size.
//!
//! Every assertion goes over the wire, through the same router the production emulator
//! mounts, so a response that is not computed from persisted state cannot pass. The tests
//! are tables: the rows differ along what the rule generalises over — how many revisions
//! the function holds, how large a page is, which states the filter names, and how the
//! function is spelled — and a single recorded listing is only one row.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::ops::types::REVISION_STATE_VALUES;
use service_com_amazonaws_lambdaweb_lambdaweb::state::MAX_MAX_RESULTS;

use super::absent_function_message::absent_function_message;
use super::revision_build_outcome::{archive, create_bucket, stage_object, BUCKET};
use super::web_function_lifecycle::{build_config, expected_arn, percent_encoded, role_arn, Emulator, Reply};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// How long a poll waits for the clock-owed build transitions before giving up.
/// Deliberately generous: the deadline is only there so a transition that never comes
/// fails the test instead of hanging it, never to time the emulator.
const SETTLE_DEADLINE: Duration = Duration::from_secs(180);

/// How long a poll sleeps between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `PUT /web-functions` with no initial revision, so a test adds exactly the revisions
/// its row calls for.
async fn create_function(emulator: &Emulator, name: &str) {
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({ "functionName": name })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
}

/// `POST /web-functions/{name}/revisions` built from the object `key`, returning the
/// `revisionId` the service minted.
async fn create_revision(emulator: &Emulator, name: &str, key: &str) -> String {
    let created = emulator
        .send(
            "POST",
            &format!("/web-functions/{name}/revisions"),
            REGION,
            Some(json!({
                "buildConfig": build_config("nodejs24.x", BUCKET, key, None),
                "serviceConfig": { "executionRoleArn": role_arn(name) },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}/{key:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
        created.status, created.body
    );
    created
        .string("revisionId")
        .unwrap_or_else(|| panic!("case {name:?}/{key:?}: revisionId is @required: {:?}", created.body))
}

/// `POST /web-functions/{functionName}/list-revisions` (model:
/// `smithy_query(query="operation", operation="ListWebFunctionRevisions")`).
async fn list_revisions(emulator: &Emulator, function: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/web-functions/{}/list-revisions", percent_encoded(function)),
            REGION,
            Some(body),
        )
        .await
}

/// Every `revisionId` of a listing page, in the order the page reported them.
fn page_ids(reply: &Reply) -> Vec<String> {
    reply
        .field("revisions")
        .and_then(Value::as_array)
        .map(|revisions| {
            revisions
                .iter()
                .map(|revision| {
                    revision
                        .get("revisionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `state` each revision of a page reported, keyed by `revisionId`.
fn page_states(reply: &Reply) -> Vec<(String, String)> {
    reply
        .field("revisions")
        .and_then(Value::as_array)
        .map(|revisions| {
            revisions
                .iter()
                .map(|revision| {
                    let member = |name: &str| {
                        revision
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("revisionId"), member("state"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of `ListWebFunctionRevisions` for `function` with `page_size`,
/// following `nextToken`, and return the ids in the order they were reported.
///
/// The walk also holds the listing to the token rule: a page that hands out a cursor is a
/// FULL page, because a cursor is handed out exactly while a matching revision remains.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    function: &str,
    page_size: usize,
    filters: Option<Value>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per revision plus spare turns: a cursor that never terminates is a bug,
    // not a reason to loop forever.
    for _ in 0..256 {
        let mut body = json!({ "maxResults": page_size });
        if let Some(filters) = &filters {
            body["filters"] = filters.clone();
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let reply = list_revisions(emulator, function, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: ListWebFunctionRevisions expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let page = page_ids(&reply);
        next_token = reply.string("nextToken");
        if next_token.is_some() {
            assert_eq!(
                page.len(),
                page_size,
                "case {case}: a page that hands out a cursor is full, got {} of {page_size} in {:?}",
                page.len(),
                reply.body
            );
        }
        assert!(
            page.len() <= page_size,
            "case {case}: a page is never longer than maxResults, got {} of {page_size}",
            page.len()
        );
        ids.extend(page);
        if next_token.is_none() {
            return ids;
        }
    }
    panic!("case {case}: ListWebFunctionRevisions never stopped handing out nextToken");
}

/// Look at the listing until no revision of `function` is `Pending` any more, and return
/// the terminal state of each.
///
/// The build transition is owed by the CLOCK, so the test waits for the CONDITION — it
/// looks again until the states have settled or the generous deadline passes.
async fn settled_states(emulator: &Emulator, function: &str) -> Vec<(String, String)> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let reply = list_revisions(emulator, function, json!({ "maxResults": MAX_MAX_RESULTS })).await;
        assert_eq!(
            reply.status, 200,
            "case {function}: ListWebFunctionRevisions expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let states = page_states(&reply);
        if states.iter().all(|(_, state)| state != "Pending") {
            return states;
        }
        assert!(
            Instant::now() < deadline,
            "case {function}: the builds never settled, last listing {states:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn every_revision_is_paged_through_exactly_once() {
    // The rows span the two axes the paging rule generalises over: how many revisions a
    // function holds (none, one, fewer than a page, more than a page, several pages'
    // worth) and how large a page is (one at a time up to the modelled maximum).
    let emulator = Emulator::new();
    let counts = [0usize, 1, 2, 5, 9];
    let page_sizes = [1usize, 2, 3, 7, MAX_MAX_RESULTS];

    for count in counts {
        let function = format!("paged-{count}");
        create_function(&emulator, &function).await;
        let mut created = Vec::new();
        for index in 0..count {
            created.push(create_revision(&emulator, &function, &format!("app-{index}.zip")).await);
        }

        for page_size in page_sizes {
            let case = format!("{count} revisions, maxResults {page_size}");
            let walked = walk_pages(&emulator, &case, &function, page_size, None).await;
            assert_eq!(
                walked, created,
                "case {case}: every revision is reported once, in creation order, got {walked:?}"
            );
            let unique: BTreeSet<&String> = walked.iter().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case}: no revision is reported twice, got {walked:?}"
            );
        }

        // `maxResults` is `@default(50)`, so a request that names none asks for the
        // largest page the model allows — every revision of these rows at once.
        let whole = list_revisions(&emulator, &function, json!({})).await;
        assert_eq!(
            whole.status, 200,
            "case {count} revisions, no maxResults: expected 200, got {} with {:?}",
            whole.status, whole.body
        );
        assert_eq!(
            page_ids(&whole), created,
            "case {count} revisions, no maxResults: the default page holds them all, got {:?}",
            whole.body
        );
        assert_eq!(
            whole.string("nextToken"),
            None,
            "case {count} revisions, no maxResults: no revision remains, got {:?}",
            whole.body
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_revisions_of_the_function_asked_for() {
    // Revisions are nested under their function, so one function's listing never
    // reports another's — whatever order the two were grown in. The rows are the two
    // functions, each with a different revision count.
    let emulator = Emulator::new();
    create_function(&emulator, "fn-left").await;
    create_function(&emulator, "fn-right").await;
    let mut expected = vec![(String::from("fn-left"), Vec::new()), (String::from("fn-right"), Vec::new())];
    for round in 0..3 {
        for (index, (function, ids)) in expected.iter_mut().enumerate() {
            if index == 1 && round == 2 {
                continue;
            }
            ids.push(create_revision(&emulator, function, &format!("app-{round}.zip")).await);
        }
    }

    for (function, ids) in &expected {
        let reply = list_revisions(&emulator, function, json!({ "maxResults": MAX_MAX_RESULTS })).await;
        assert_eq!(
            reply.status, 200,
            "case {function:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            &page_ids(&reply), ids,
            "case {function:?}: only this function's revisions are reported, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_state_filter_narrows_the_revision_listing() {
    // `filters` supports the name `state` only (source: smithy_query doc for
    // ListWebFunctionRevisionsRequest$filters), and a revision matches when its state
    // is among the filter's values. The rows are the states a revision really settles
    // in: this function's builds are a mix, because two of its packages are archives
    // the build reads and two are not.
    let emulator = Emulator::with_object_store();
    create_bucket(&emulator).await;
    stage_object(
        &emulator,
        "builds.zip",
        archive(&[("index.mjs", b"export const handler = () => 1;")]),
    )
    .await;
    stage_object(&emulator, "no-archive.zip", b"not a zip archive".to_vec()).await;

    let function = "mixed-builds";
    create_function(&emulator, function).await;
    // Two keys that build and two that do not, interleaved, so neither group is a
    // contiguous run of the stored order.
    let active_a = create_revision(&emulator, function, "builds.zip").await;
    let failed_a = create_revision(&emulator, function, "no-archive.zip").await;
    let active_b = create_revision(&emulator, function, "builds.zip").await;
    // A key the bucket holds nothing under: Amazon S3 answers that it will not serve
    // the object, so this build fails too.
    let failed_b = create_revision(&emulator, function, "absent.zip").await;

    let settled = settled_states(&emulator, function).await;
    let of_state = |wanted: &str| -> Vec<String> {
        settled
            .iter()
            .filter(|(_, state)| state == wanted)
            .map(|(id, _)| id.clone())
            .collect()
    };
    assert_eq!(
        of_state("Active"),
        vec![active_a.clone(), active_b.clone()],
        "the two archive-backed builds settle Active, got {settled:?}"
    );
    assert_eq!(
        of_state("Failed"),
        vec![failed_a.clone(), failed_b.clone()],
        "the two packages the build cannot read settle Failed, got {settled:?}"
    );

    let all: Vec<String> = settled.iter().map(|(id, _)| id.clone()).collect();
    let rows: Vec<(Vec<&str>, Vec<String>)> = vec![
        (vec!["Active"], of_state("Active")),
        (vec!["Failed"], of_state("Failed")),
        (vec!["Pending"], Vec::new()),
        (vec!["Active", "Failed"], all.clone()),
        (vec!["Pending", "Active"], of_state("Active")),
        (vec!["Pending", "Failed"], of_state("Failed")),
        (vec!["Active", "Failed", "Pending"], all.clone()),
    ];
    for (values, expected) in rows {
        let filters = json!([{ "name": "state", "values": values }]);
        let reply = list_revisions(
            &emulator,
            function,
            json!({ "maxResults": MAX_MAX_RESULTS, "filters": filters.clone() }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {values:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            page_ids(&reply), expected,
            "case {values:?}: expected {expected:?}, got {:?}",
            reply.body
        );

        // A narrowed listing paginates like any other: a page at a time reports the
        // same matches in the same order.
        let case = format!("{values:?} one at a time");
        let walked = walk_pages(&emulator, &case, function, 1, Some(filters)).await;
        assert_eq!(
            walked, expected,
            "case {case}: expected {expected:?}, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn every_modelled_revision_state_is_an_accepted_filter_value_and_nothing_else_is() {
    // The accepted values are the `RevisionState` enum's, all of them (source:
    // `smithy_query(query="shape", shape="RevisionState")` — Active, Failed, Pending,
    // mirrored by `REVISION_STATE_VALUES`), so the catalogue is driven from that list
    // rather than from the states this function happens to be in.
    let emulator = Emulator::new();
    let function = "filter-values";
    create_function(&emulator, function).await;
    let revision = create_revision(&emulator, function, "app.zip").await;
    let settled = settled_states(&emulator, function).await;
    let state_of_revision = settled
        .iter()
        .find(|(id, _)| id == &revision)
        .map(|(_, state)| state.clone())
        .unwrap_or_else(|| panic!("the created revision is listed, got {settled:?}"));

    // Every modelled value is accepted, and each reports exactly the revisions in that
    // state: the one revision for the state it settled in, nothing for the others.
    for value in REVISION_STATE_VALUES {
        let reply = list_revisions(
            &emulator,
            function,
            json!({ "filters": [{ "name": "state", "values": [value] }] }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {value:?}: a modelled RevisionState is accepted, got {} with {:?}",
            reply.status, reply.body
        );
        let expected: Vec<String> = if *value == state_of_revision {
            vec![revision.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            page_ids(&reply), expected,
            "case {value:?}: the revision settled {state_of_revision}, so expected \
             {expected:?}, got {:?}",
            reply.body
        );
    }

    // An unsupported filter NAME, and a value that is not a `RevisionState` — including
    // `Deleting`, which is a `FunctionState` but no state a revision ever has — are
    // refused as a validation failure, not served as an empty page.
    let refused = [
        json!({ "name": "revisionId", "values": ["00000000-0000-0000-0000-000000000001"] }),
        json!({ "name": "State", "values": ["Active"] }),
        json!({ "name": "states", "values": ["Active"] }),
        json!({ "name": "state", "values": ["active"] }),
        json!({ "name": "state", "values": ["ACTIVE"] }),
        json!({ "name": "state", "values": ["Deleting"] }),
        json!({ "name": "state", "values": ["NotAState"] }),
        json!({ "name": "state", "values": ["Active", "NotAState"] }),
    ];
    for filter in refused {
        let reply = list_revisions(&emulator, function, json!({ "filters": [filter.clone()] })).await;
        assert_eq!(
            reply.status, 400,
            "case {filter:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error(),
            "ValidationException",
            "case {filter:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_function_can_be_named_by_name_or_by_arn_and_an_unknown_one_is_not_found() {
    // The member's documentation says the caller "can specify either of the following:
    // The function name, for example my-function. The function ARN." (source:
    // smithy_query doc for ListWebFunctionRevisionsRequest$functionName). The rows are
    // the two documented spellings of ONE function, and then the same two spellings of
    // a function nothing stored.
    let emulator = Emulator::new();
    let function = "named-either-way";
    create_function(&emulator, function).await;
    let created = vec![
        create_revision(&emulator, function, "app-0.zip").await,
        create_revision(&emulator, function, "app-1.zip").await,
    ];

    for spelling in [function.to_string(), expected_arn(REGION, function)] {
        let reply = list_revisions(&emulator, &spelling, json!({ "maxResults": MAX_MAX_RESULTS })).await;
        assert_eq!(
            reply.status, 200,
            "case {spelling:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            page_ids(&reply), created,
            "case {spelling:?}: both spellings name one function, got {:?}",
            reply.body
        );
    }

    for spelling in [
        "no-such-function".to_string(),
        expected_arn(REGION, "no-such-function"),
    ] {
        let reply = list_revisions(&emulator, &spelling, json!({})).await;
        assert_eq!(
            reply.status, 404,
            "case {spelling:?}: expected 404, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error(),
            "ResourceNotFoundException",
            "case {spelling:?}: expected ResourceNotFoundException, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            Some(absent_function_message(&spelling)),
            "case {spelling:?}: the one sentence an absent function is reported with, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    // The rows all satisfy the model's `NextToken` pattern `^[a-f0-9-]{36}$`, so the
    // boundary passes them; none of them is a cursor this service minted.
    let emulator = Emulator::new();
    let function = "cursor-rules";
    create_function(&emulator, function).await;
    let created = [
        create_revision(&emulator, function, "app-0.zip").await,
        create_revision(&emulator, function, "app-1.zip").await,
    ];

    for token in [
        "aaaaaaaaa-aaa-aaaa-aaaa-aaaaaaaaaaaa",
        "------------------------------------",
        "0000000000000000000000000000000000-0",
        "0-0000000000000000000000000000000000",
    ] {
        let reply = list_revisions(
            &emulator,
            function,
            json!({ "maxResults": 1, "nextToken": token }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }

    // A cursor this service DID mint resumes at the next revision.
    let first = list_revisions(&emulator, function, json!({ "maxResults": 1 })).await;
    assert_eq!(
        page_ids(&first),
        created[..1].to_vec(),
        "case minted: the first page holds the first revision, got {:?}",
        first.body
    );
    let token = first
        .string("nextToken")
        .expect("a second revision remains, so a cursor is expected");
    let second = list_revisions(
        &emulator,
        function,
        json!({ "maxResults": 1, "nextToken": token }),
    )
    .await;
    assert_eq!(
        page_ids(&second),
        created[1..].to_vec(),
        "case minted: the cursor resumes at the next revision, got {:?}",
        second.body
    );
    assert_eq!(
        second.string("nextToken"),
        None,
        "case minted: nothing remains after the last revision, got {:?}",
        second.body
    );
}
