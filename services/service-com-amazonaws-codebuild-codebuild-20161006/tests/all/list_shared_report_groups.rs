// SPDX-License-Identifier: CC0-1.0

//! `ListSharedReportGroups`, over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListSharedReportGroups` on awsJson1_1.
//!
//! RULE: the listing is the CONSUMER's side of a one-directional relationship — it
//! reports the ARNs of exactly those report groups that ANOTHER account owns and whose
//! resource-based policy grants this account, never one this account owns, however many
//! policies that one carries — ordered by the criterion and direction the request asks
//! for, cut into pages of at most `maxResults` entries that `nextToken` resumes; a policy
//! held for another resource kind lists no report group; and the read leaves the store
//! exactly as it found it.
//!
//! Who OWNS a report group is the account field of the ARN it is stored with, so a group
//! another account owns is seeded by storing it under that account's ARN, and the sharing
//! record straight into the service's state beside it — seeding is what proves this
//! listing is computed from the store rather than from the request, and the operation that
//! stores a policy over the wire (`PutResourcePolicy`) is covered by `resource_policy`.
//!
//! The harness (`call`, `emulator_on`, `error_type`, `REGION`) is the one
//! `project_build_lifecycle` already drives this service with, and the report-group
//! request builder is `report_group_lifecycle`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredResourcePolicy,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, emulator_on, error_type, REGION};
use super::report_group_lifecycle::create_report_group_body;

/// An account that is NOT the one this emulator serves: the owner of every report group
/// below that is shared WITH this account.
const ANOTHER_ACCOUNT: &str = "222222222222";

/// A third account, which neither owns nor is granted anything here — what a grant
/// written for somebody else names.
const THIRD_ACCOUNT: &str = "333333333333";

/// The account this emulator serves, read out of an ARN it mints rather than written out
/// here, so the table below cannot drift from the account the handlers compare against.
fn this_account() -> String {
    let minted = state::report_group_arn(REGION, "account-probe");
    state::arn_account(&minted)
        .unwrap_or_else(|| panic!("an ARN this emulator mints names its account: {minted}"))
        .to_string()
}

/// The ARN a report group named `name` carries in `account`:
/// `arn:aws:codebuild:<region>:<account>:report-group/<name>`, the form
/// `state::report_group_arn` mints for this account's own.
fn report_group_arn_in(account: &str, name: &str) -> String {
    format!("arn:aws:codebuild:{REGION}:{account}:report-group/{name}")
}

/// A resource-based policy document granting `principal` read access to `arn` — the
/// read-only consumer actions the CodeBuild user guide's "Sharing a report group"
/// describes.
fn sharing_policy(arn: &str, principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": ["codebuild:BatchGetReportGroups", "codebuild:BatchGetReports"],
            "Resource": arn,
        }],
    })
    .to_string()
}

/// Attach a policy granting `principal` to `arn` in the store the handlers read.
fn share(engine: &engine::InMemoryEngine, arn: &str, principal: &str) {
    attach(engine, arn, sharing_policy(arn, principal));
}

/// Attach `document` to `arn` in the store the handlers read, whatever that document
/// says — the seam the rows that vary the DOCUMENT rather than the principal use.
fn attach(engine: &engine::InMemoryEngine, arn: &str, document: String) {
    engine
        .state::<CodeBuildState>()
        .lock()
        .resource_policies
        .insert(
            arn.to_string(),
            StoredResourcePolicy {
                policy: document,
                last_modified_millis: state::now_millis(),
            },
        );
}

/// Hand the report group `name` over to `account`, as the store holds a group that
/// account owns, and answer the ARN it is now stored with: the stored ARN is the whole
/// record of who owns a resource.
fn owned_by(engine: &engine::InMemoryEngine, account: &str, name: &str) -> String {
    let arn = report_group_arn_in(account, name);
    let store = engine.state::<CodeBuildState>();
    let mut held = store.lock();
    let stored = held
        .report_groups
        .get_mut(name)
        .unwrap_or_else(|| panic!("the store holds {name:?} after CreateReportGroup"));
    stored.report_group.arn = Some(arn.clone());
    arn
}

/// Create `names` as report groups of the account and hand back the ARN each was stored
/// with, read from the service's own answer rather than assembled here.
///
/// `pub(crate)` because `tests/all/list_report_groups.rs` seeds the listing of ALL report
/// groups the same way rather than growing a second copy of this helper.
pub(crate) async fn create_report_groups(router: &MultiServiceRouter, names: &[&str]) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let (status, body) = call(
            router,
            "CreateReportGroup",
            create_report_group_body(name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateReportGroup {name:?}: expected 200, got {status} with {body}"
        );
        arns.push(
            body["reportGroup"]["arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("CreateReportGroup {name:?} must report an ARN, got {body}")
                })
                .to_string(),
        );
    }
    arns
}

/// Call `ListSharedReportGroups` with `request`, asserting it succeeded, and hand back
/// the body.
async fn list(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "ListSharedReportGroups", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListSharedReportGroups {request}: expected 200, got {status} with {body}"
    );
    body
}

/// The `reportGroups` member of a listing, as a vector of ARNs — asserting it was stated
/// as a list at all.
///
/// `pub(crate)` because both report-group listings state their result in a `reportGroups`
/// member of the same shape, so `tests/all/list_report_groups.rs` reads one through this.
pub(crate) fn listed(body: &Value, case: &str) -> Vec<String> {
    let entries = body["reportGroups"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: reportGroups must be stated as a list, got {body}")
    });
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every entry is an ARN, got {body}"))
                .to_string()
        })
        .collect()
}

/// One seeded report group of a row of the table below: its name, the account that owns
/// it, the principal the policy attached to it grants (`None`: no policy at all), and
/// whether the shared listing must report it.
struct Seeded {
    name: &'static str,
    owner: String,
    principal: Option<String>,
    shared: bool,
}

/// Seed `groups` into a fresh emulator and hand back the router, the engine and the ARN
/// each group ended up stored with.
async fn seed(
    groups: &[Seeded],
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let names: Vec<&str> = groups.iter().map(|group| group.name).collect();
    create_report_groups(&router, &names).await;
    let mut arns = Vec::new();
    for group in groups {
        let arn = owned_by(&engine, &group.owner, group.name);
        if let Some(principal) = group.principal.as_deref() {
            share(&engine, &arn, principal);
        }
        arns.push(arn);
    }
    (router, engine, arns)
}

/// RULE: exactly the report groups ANOTHER account owns and has shared with this account
/// are listed — a group this account owns never is, whatever policy it carries — and the
/// account's own listing reports exactly the groups it owns, so the two listings
/// partition the store.
#[tokio::test]
async fn only_the_report_groups_another_account_shared_with_this_one_are_listed() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");
    let another_root = format!("arn:aws:iam::{ANOTHER_ACCOUNT}:root");
    let third_root = format!("arn:aws:iam::{THIRD_ACCOUNT}:root");

    // The rows differ along the direction of the sharing relationship and along the
    // principal a grant names: nothing shared either way, this account's own group
    // policied for itself and for somebody else, one and several groups another account
    // shared with this one, a foreign group granted to a third account, a foreign group
    // carrying no policy at all, and each form a grant names this account by.
    let rows: Vec<(&str, Vec<Seeded>)> = vec![
        (
            "an account that shares nothing and is shared nothing",
            vec![
                Seeded {
                    name: "alpha",
                    owner: this.clone(),
                    principal: None,
                    shared: false,
                },
                Seeded {
                    name: "bravo",
                    owner: this.clone(),
                    principal: None,
                    shared: false,
                },
            ],
        ),
        (
            "the owner's own group, policied for its own root principal",
            vec![Seeded {
                name: "alpha",
                owner: this.clone(),
                principal: Some(own_root.clone()),
                shared: false,
            }],
        ),
        (
            "the owner's own group, shared OUT to another account",
            vec![Seeded {
                name: "alpha",
                owner: this.clone(),
                principal: Some(another_root.clone()),
                shared: false,
            }],
        ),
        (
            "one group another account shared with this one",
            vec![Seeded {
                name: "alpha",
                owner: ANOTHER_ACCOUNT.to_string(),
                principal: Some(own_root.clone()),
                shared: true,
            }],
        ),
        (
            "two shared with this one, beside an owned group policied for itself",
            vec![
                Seeded {
                    name: "alpha",
                    owner: ANOTHER_ACCOUNT.to_string(),
                    principal: Some(own_root.clone()),
                    shared: true,
                },
                Seeded {
                    name: "bravo",
                    owner: this.clone(),
                    principal: Some(own_root.clone()),
                    shared: false,
                },
                Seeded {
                    name: "charlie",
                    owner: THIRD_ACCOUNT.to_string(),
                    principal: Some(own_root.clone()),
                    shared: true,
                },
            ],
        ),
        (
            "a foreign group whose grant names a third account",
            vec![Seeded {
                name: "alpha",
                owner: ANOTHER_ACCOUNT.to_string(),
                principal: Some(third_root.clone()),
                shared: false,
            }],
        ),
        (
            "a foreign group carrying no policy at all",
            vec![Seeded {
                name: "alpha",
                owner: ANOTHER_ACCOUNT.to_string(),
                principal: None,
                shared: false,
            }],
        ),
        (
            "every principal form a grant names this account by",
            vec![
                Seeded {
                    name: "wildcard",
                    owner: ANOTHER_ACCOUNT.to_string(),
                    principal: Some("*".to_string()),
                    shared: true,
                },
                Seeded {
                    name: "bare-account-id",
                    owner: ANOTHER_ACCOUNT.to_string(),
                    principal: Some(this.clone()),
                    shared: true,
                },
                Seeded {
                    name: "root-arn",
                    owner: ANOTHER_ACCOUNT.to_string(),
                    principal: Some(own_root.clone()),
                    shared: true,
                },
                Seeded {
                    name: "role-arn",
                    owner: ANOTHER_ACCOUNT.to_string(),
                    principal: Some(format!("arn:aws:iam::{this}:role/consumer")),
                    shared: true,
                },
            ],
        ),
    ];

    for (case, groups) in rows {
        let (router, engine, arns) = seed(&groups).await;
        // A policy on the other resource kind CodeBuild shares, owned elsewhere and
        // granted to this account: it names no report group, so it must list none.
        share(
            &engine,
            &format!("arn:aws:codebuild:{REGION}:{ANOTHER_ACCOUNT}:project/shared-project"),
            &own_root,
        );

        let mut expected: Vec<String> = groups
            .iter()
            .zip(&arns)
            .filter(|(group, _)| group.shared)
            .map(|(_, arn)| arn.clone())
            .collect();
        expected.sort();
        let body = list(&router, json!({})).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?}: expected the report groups shared with this account \
             {expected:?}, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page holds every entry, so no nextToken may be stated, got {body}"
        );

        // The mirror: the account's OWN listing reports exactly the groups it owns, so a
        // group is in one listing or the other and never in both.
        let mut owned: Vec<String> = groups
            .iter()
            .zip(&arns)
            .filter(|(group, _)| group.owner == this)
            .map(|(_, arn)| arn.clone())
            .collect();
        owned.sort();
        let (status, held_body) = call(&router, "ListReportGroups", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListReportGroups must succeed, got {status} with {held_body}"
        );
        assert_eq!(
            listed(&held_body, case),
            owned,
            "case {case:?}: the account's own listing reports exactly the groups it owns \
             {owned:?}, got {held_body}"
        );

        // The read is reporting only: a second call answers the same thing and the store
        // is untouched.
        let again = list(&router, json!({})).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must answer the same listing, got {again}"
        );
        let store = engine.state::<CodeBuildState>();
        let held = store.lock();
        let policies = groups
            .iter()
            .filter(|group| group.principal.is_some())
            .count()
            + 1;
        assert_eq!(
            (held.report_groups.len(), held.resource_policies.len()),
            (groups.len(), policies),
            "case {case:?}: a read must neither add to nor drop from the store"
        );
    }
}

/// RULE: a report group another account owns is reported only while the document attached
/// to it ALLOWS something to a principal of this account — so a document that grants this
/// account nothing lists nothing, whatever else it says.
#[tokio::test]
async fn a_document_that_allows_this_account_nothing_lists_nothing() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");

    // The rows differ along WHY the document grants this account nothing: it denies
    // instead of allowing, it allows somebody else, it names no principal, it carries no
    // statement, it is the smallest JSON object there is, and it allows this account
    // under a principal form that names a service rather than an account.
    let documents: Vec<(&str, String)> = vec![
        (
            "a statement that denies this account",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"AWS": own_root.clone()},
                    "Action": ["codebuild:BatchGetReportGroups"],
                }],
            })
            .to_string(),
        ),
        (
            "an allow for a third account only",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{THIRD_ACCOUNT}:root")},
                    "Action": ["codebuild:BatchGetReportGroups"],
                }],
            })
            .to_string(),
        ),
        (
            "an allow naming no principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["codebuild:BatchGetReportGroups"],
                }],
            })
            .to_string(),
        ),
        (
            "no statement at all",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        ("the smallest JSON object", "{}".to_string()),
        (
            "a principal that names a service",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "codebuild.amazonaws.com"},
                    "Action": ["codebuild:BatchGetReportGroups"],
                }],
            })
            .to_string(),
        ),
    ];

    for (case, document) in documents {
        let (router, engine, arns) = seed(&[Seeded {
            name: "foreign-group",
            owner: ANOTHER_ACCOUNT.to_string(),
            principal: None,
            shared: false,
        }])
        .await;
        attach(&engine, &arns[0], document.clone());

        let body = list(&router, json!({})).await;
        assert_eq!(
            listed(&body, case),
            Vec::<String>::new(),
            "case {case:?}: {document} grants this account nothing, so nothing is listed, \
             got {body}"
        );
    }

    // The control: the same foreign group with a document that DOES allow this account
    // is listed, so the rows above fail for the reason they name rather than because the
    // seeding never worked.
    let (router, engine, arns) = seed(&[Seeded {
        name: "foreign-group",
        owner: ANOTHER_ACCOUNT.to_string(),
        principal: None,
        shared: false,
    }])
    .await;
    share(&engine, &arns[0], &own_root);
    let body = list(&router, json!({})).await;
    assert_eq!(
        listed(&body, "an allow for this account"),
        vec![arns[0].clone()],
        "the control row must be listed, got {body}"
    );
}

/// RULE: the listing is ordered by the requested criterion in the requested direction —
/// by ARN or by when each shared report group was last changed, ascending or descending —
/// and an absent member takes the ascending-by-ARN default.
#[tokio::test]
async fn the_listing_is_ordered_by_the_requested_criterion_and_direction() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");
    let names = ["ord-a", "ord-b", "ord-c", "ord-d"];
    // Four report groups another account owns, every one of them shared with this one.
    let groups: Vec<Seeded> = names
        .iter()
        .map(|name| Seeded {
            name,
            owner: ANOTHER_ACCOUNT.to_string(),
            principal: Some(own_root.clone()),
            shared: true,
        })
        .collect();
    let (router, engine, arns) = seed(&groups).await;

    // The instant each group was last changed at, seeded as a fixed value rather than
    // taken from the clock, so the modified order DISAGREES with the ARN order and each
    // row below can only pass for its own criterion. `ord-c` and `ord-d` carry the same
    // instant, which is what the ARN tie-break is for.
    let stamps = [
        ("ord-a", 1_700_000_300_000_i64),
        ("ord-b", 1_700_000_100_000),
        ("ord-c", 1_700_000_200_000),
        ("ord-d", 1_700_000_200_000),
    ];
    {
        let store = engine.state::<CodeBuildState>();
        let mut held = store.lock();
        for (name, millis) in stamps {
            let stored = held
                .report_groups
                .get_mut(name)
                .unwrap_or_else(|| panic!("the store holds {name:?} after CreateReportGroup"));
            stored.last_modified_millis = millis;
            stored.report_group.last_modified = Some(state::datetime(millis));
        }
    }

    let by_arn: Vec<String> = {
        let mut sorted = arns.clone();
        sorted.sort();
        sorted
    };
    let by_modified_time: Vec<String> = {
        let mut stamped: Vec<(i64, String)> = stamps
            .iter()
            .map(|(name, millis)| (*millis, report_group_arn_in(ANOTHER_ACCOUNT, name)))
            .collect();
        stamped.sort();
        stamped.into_iter().map(|(_, arn)| arn).collect()
    };
    assert_ne!(
        by_arn, by_modified_time,
        "the two criteria must disagree for this table to mean anything: {by_arn:?}"
    );

    let descending = |order: &[String]| -> Vec<String> {
        let mut reversed = order.to_vec();
        reversed.reverse();
        reversed
    };

    // The rows span both values of `SharedResourceSortByType` (ARN | MODIFIED_TIME) and
    // both of `SortOrderType` (ASCENDING | DESCENDING), plus each member absent.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("neither member", json!({}), by_arn.clone()),
        (
            "ARN, ascending",
            json!({"sortBy": "ARN", "sortOrder": "ASCENDING"}),
            by_arn.clone(),
        ),
        (
            "ARN, descending",
            json!({"sortBy": "ARN", "sortOrder": "DESCENDING"}),
            descending(&by_arn),
        ),
        ("ARN, no order", json!({"sortBy": "ARN"}), by_arn.clone()),
        (
            "MODIFIED_TIME, ascending",
            json!({"sortBy": "MODIFIED_TIME", "sortOrder": "ASCENDING"}),
            by_modified_time.clone(),
        ),
        (
            "MODIFIED_TIME, descending",
            json!({"sortBy": "MODIFIED_TIME", "sortOrder": "DESCENDING"}),
            descending(&by_modified_time),
        ),
        (
            "MODIFIED_TIME, no order",
            json!({"sortBy": "MODIFIED_TIME"}),
            by_modified_time.clone(),
        ),
        (
            "only an order",
            json!({"sortOrder": "DESCENDING"}),
            descending(&by_arn),
        ),
    ];

    for (case, request, expected) in rows {
        let body = list(&router, request.clone()).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?} ({request}): expected {expected:?}, got {body}"
        );
    }
}

/// RULE: the listing is cut into pages of at most `maxResults` entries, `nextToken`
/// resumes the next page, the last page states none, and walking the pages yields the
/// whole ordering exactly once.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_resumes_at_the_next_token() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");
    let names = ["paged-0", "paged-1", "paged-2", "paged-3", "paged-4"];
    let groups: Vec<Seeded> = names
        .iter()
        .map(|name| Seeded {
            name,
            owner: ANOTHER_ACCOUNT.to_string(),
            principal: Some(own_root.clone()),
            shared: true,
        })
        .collect();
    let (router, _engine, arns) = seed(&groups).await;
    let mut expected = arns.clone();
    expected.sort();

    // The sizes span one entry per page, a size that divides the count, one that does
    // not, the exact count, and `PageSize`'s published maximum of 100 (from
    // `smithy_query(query="operation",operation="ListSharedReportGroups")`: @range(min:
    // 1, max: 100)), where one page holds everything.
    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let (Value::Object(target), Some(token)) = (&mut request, token.as_ref()) {
                target.insert("nextToken".to_string(), json!(token));
            }
            let case = format!("maxResults {page_size}, page {pages}");
            let body = list(&router, request).await;
            let page = listed(&body, &case);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: a page holds 1..={page_size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len(),
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {page_size}: walking the pages must yield every shared report group \
             once, in order, got {walked:?}"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {page_size}: unexpected page count for {} entries",
            expected.len()
        );
    }

    // The token is the entry the page opens at, so the one the FIRST page reported
    // resumes at the second entry.
    let first = list(&router, json!({"maxResults": 1})).await;
    let resumed = list(
        &router,
        json!({"maxResults": 1, "nextToken": first["nextToken"]}),
    )
    .await;
    assert_eq!(
        listed(&resumed, "resumed"),
        vec![expected[1].clone()],
        "the reported token must resume at the next entry, got {resumed}"
    );

    // A token naming nothing in the listing is a bad input, not an empty page.
    for bogus in [
        "not-a-token".to_string(),
        report_group_arn_in(ANOTHER_ACCOUNT, "never-created"),
        state::report_group_arn(REGION, "paged-0"),
    ] {
        let (status, body) = call(
            &router,
            "ListSharedReportGroups",
            json!({"nextToken": bogus.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: an unresolvable nextToken is refused, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {bogus:?}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
    }
}
