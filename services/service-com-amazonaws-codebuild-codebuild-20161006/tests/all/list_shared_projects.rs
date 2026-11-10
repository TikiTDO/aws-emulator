// SPDX-License-Identifier: CC0-1.0

//! `ListSharedProjects`, over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListSharedProjects` on awsJson1_1.
//!
//! RULE: the listing is the CONSUMER's side of a one-directional relationship — it
//! reports the ARNs of exactly those build projects that ANOTHER account owns and whose
//! resource-based policy grants this account, never one this account owns, however many
//! policies that one carries — ordered by the criterion and direction the request asks
//! for, cut into pages of at most `maxResults` entries that `nextToken` resumes; a
//! project's `projectVisibility` is a different grant and never makes a project shared;
//! a policy held for another resource kind lists no project; and the read leaves the
//! store exactly as it found it.
//!
//! Who OWNS a project is the account field of the ARN it is stored with, so a project
//! another account owns is seeded by storing it under that account's ARN, and the sharing
//! record straight into the service's state beside it — through the same seam
//! `report_group_reads` seeds a report group with. Seeding is what proves this listing is
//! computed from the store rather than from the request, and the operation that stores a
//! policy over the wire (`PutResourcePolicy`) is covered by `resource_policy` beside this
//! suite, including the shared listing it feeds.
//!
//! The harness (`call`, `create_project`, `emulator_on`, `error_type`, `REGION`) is the
//! one `project_build_lifecycle` already drives this service with.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredResourcePolicy,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator_on, error_type, REGION};

/// An account that is NOT the one this emulator serves: the owner of every project below
/// that is shared WITH this account.
const ANOTHER_ACCOUNT: &str = "222222222222";

/// A third account, which neither owns nor is granted anything here — what a grant
/// written for somebody else names.
const THIRD_ACCOUNT: &str = "333333333333";

/// The account this emulator serves, read out of an ARN it mints rather than written out
/// here, so the table below cannot drift from the account the handlers compare against.
fn this_account() -> String {
    let minted = state::project_arn(REGION, "account-probe");
    state::arn_account(&minted)
        .unwrap_or_else(|| panic!("an ARN this emulator mints names its account: {minted}"))
        .to_string()
}

/// The ARN a project named `name` carries in `account`:
/// `arn:aws:codebuild:<region>:<account>:project/<name>`, the form `state::project_arn`
/// mints for this account's own.
fn project_arn_in(account: &str, name: &str) -> String {
    format!("arn:aws:codebuild:{REGION}:{account}:project/{name}")
}

/// A resource-based policy document granting `principal` read access to `arn` — one of
/// the grants the CodeBuild user guide's "Sharing a Project" describes.
fn sharing_policy(arn: &str, principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": ["codebuild:BatchGetProjects", "codebuild:ListBuildsForProject"],
            "Resource": arn,
        }],
    })
    .to_string()
}

/// Attach a policy granting `principal` to `arn` in the store the handlers read.
fn share(engine: &engine::InMemoryEngine, arn: &str, principal: &str) {
    engine.state::<CodeBuildState>().lock().resource_policies.insert(
        arn.to_string(),
        StoredResourcePolicy {
            policy: sharing_policy(arn, principal),
            last_modified_millis: state::now_millis(),
        },
    );
}

/// Hand the project `name` over to `account`, as the store holds a project that account
/// owns, and answer the ARN it is now stored with: the stored ARN is the whole record of
/// who owns a resource.
fn owned_by(engine: &engine::InMemoryEngine, account: &str, name: &str) -> String {
    let arn = project_arn_in(account, name);
    let store = engine.state::<CodeBuildState>();
    let mut held = store.lock();
    let stored = held
        .projects
        .get_mut(name)
        .unwrap_or_else(|| panic!("the store holds {name:?} after CreateProject"));
    stored.project.arn = Some(arn.clone());
    arn
}

/// Call `ListSharedProjects` with `request`, asserting it succeeded, and hand back the
/// body.
async fn list(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "ListSharedProjects", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListSharedProjects {request}: expected 200, got {status} with {body}"
    );
    body
}

/// The `projects` member of a listing, as a vector of entries — asserting it was stated
/// as a list at all. The shared listing states ARNs and the account's own listing states
/// names, so both are read through here.
fn listed(body: &Value, case: &str) -> Vec<String> {
    let entries = body["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: projects must be stated as a list, got {body}"));
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every entry is a string, got {body}"))
                .to_string()
        })
        .collect()
}

/// Create `names` as projects of the account and hand back the ARN each was stored
/// with, read from the service's own answer.
async fn create_projects(router: &MultiServiceRouter, names: &[&str]) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let project = create_project(router, name, json!({})).await;
        arns.push(
            project["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateProject {name:?} must report an ARN, got {project}"))
                .to_string(),
        );
    }
    arns
}

/// One seeded project of a row of the table below: its name, the account that owns it,
/// the principal the policy attached to it grants (`None`: no policy at all), the
/// visibilities it is moved through before anything else happens to it, and whether the
/// shared listing must report it.
struct Seeded {
    name: &'static str,
    owner: String,
    principal: Option<String>,
    visibilities: &'static [&'static str],
    shared: bool,
}

impl Seeded {
    /// A project of this account, private and carrying no policy — what each row varies
    /// from.
    fn held(name: &'static str, owner: &str) -> Self {
        Self {
            name,
            owner: owner.to_string(),
            principal: None,
            visibilities: &[],
            shared: false,
        }
    }

    /// The same project with a policy granting `principal` attached.
    fn granting(mut self, principal: &str, shared: bool) -> Self {
        self.principal = Some(principal.to_string());
        self.shared = shared;
        self
    }

    /// The same project moved through `visibilities` by `UpdateProjectVisibility`.
    fn published(mut self, visibilities: &'static [&'static str]) -> Self {
        self.visibilities = visibilities;
        self
    }
}

/// Seed `projects` into a fresh emulator and hand back the router, the engine and the ARN
/// each project ended up stored with.
async fn seed(
    projects: &[Seeded],
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let names: Vec<&str> = projects.iter().map(|project| project.name).collect();
    let own_arns = create_projects(&router, &names).await;

    // Visibility is moved while the project is still this account's own, which is the
    // only account that can publish it.
    for (project, own_arn) in projects.iter().zip(&own_arns) {
        for visibility in project.visibilities {
            let (status, body) = call(
                &router,
                "UpdateProjectVisibility",
                json!({"projectArn": own_arn, "projectVisibility": visibility}),
            )
            .await;
            assert_eq!(
                status, 200,
                "setting {own_arn} to {visibility} must succeed, got {status} with {body}"
            );
        }
    }

    let mut arns = Vec::new();
    for project in projects {
        let arn = owned_by(&engine, &project.owner, project.name);
        if let Some(principal) = project.principal.as_deref() {
            share(&engine, &arn, principal);
        }
        arns.push(arn);
    }
    (router, engine, arns)
}

/// RULE: exactly the projects ANOTHER account owns and has shared with this account are
/// listed — a project this account owns never is, whatever policy it carries and whatever
/// its visibility — and the account's own listing reports exactly the projects it owns,
/// so the two listings partition the store.
#[tokio::test]
async fn only_the_projects_another_account_shared_with_this_one_are_listed() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");
    let another_root = format!("arn:aws:iam::{ANOTHER_ACCOUNT}:root");
    let third_root = format!("arn:aws:iam::{THIRD_ACCOUNT}:root");

    // The rows differ along the direction of the sharing relationship, along the
    // principal a grant names, and along the visibility of the projects on either side,
    // which the rule says is irrelevant.
    let rows: Vec<(&str, Vec<Seeded>)> = vec![
        (
            "an account that shares nothing and is shared nothing",
            vec![
                Seeded::held("alpha", &this),
                Seeded::held("bravo", &this),
            ],
        ),
        (
            "the owner's own project, policied for its own root principal",
            vec![Seeded::held("alpha", &this).granting(&own_root, false)],
        ),
        (
            "the owner's own project, shared OUT to another account",
            vec![Seeded::held("alpha", &this).granting(&another_root, false)],
        ),
        (
            "one project another account shared with this one",
            vec![Seeded::held("alpha", ANOTHER_ACCOUNT).granting(&own_root, true)],
        ),
        (
            "two shared with this one, beside an owned project policied for itself",
            vec![
                Seeded::held("alpha", ANOTHER_ACCOUNT).granting(&own_root, true),
                Seeded::held("bravo", &this).granting(&own_root, false),
                Seeded::held("charlie", THIRD_ACCOUNT).granting(&own_root, true),
            ],
        ),
        (
            "a foreign project whose grant names a third account",
            vec![Seeded::held("alpha", ANOTHER_ACCOUNT).granting(&third_root, false)],
        ),
        (
            "a foreign project carrying no policy at all",
            vec![Seeded::held("alpha", ANOTHER_ACCOUNT)],
        ),
        (
            "a public project of this account, shared out and not",
            vec![
                Seeded::held("alpha", &this).published(&["PUBLIC_READ"]),
                Seeded::held("bravo", &this)
                    .published(&["PUBLIC_READ"])
                    .granting(&another_root, false),
            ],
        ),
        (
            "a project published and taken private again, on either side",
            vec![
                Seeded::held("alpha", &this)
                    .published(&["PUBLIC_READ", "PRIVATE"])
                    .granting(&own_root, false),
                Seeded::held("bravo", ANOTHER_ACCOUNT)
                    .published(&["PUBLIC_READ", "PRIVATE"])
                    .granting(&own_root, true),
            ],
        ),
        (
            "every principal form a grant names this account by",
            vec![
                Seeded::held("wildcard", ANOTHER_ACCOUNT).granting("*", true),
                Seeded::held("bare-account-id", ANOTHER_ACCOUNT).granting(&this, true),
                Seeded::held("root-arn", ANOTHER_ACCOUNT).granting(&own_root, true),
                Seeded::held("role-arn", ANOTHER_ACCOUNT)
                    .granting(&format!("arn:aws:iam::{this}:role/consumer"), true),
            ],
        ),
    ];

    for (case, projects) in rows {
        let (router, engine, arns) = seed(&projects).await;
        // A policy on the other resource kind CodeBuild shares, owned elsewhere and
        // granted to this account: it names no project, so it must list none.
        share(
            &engine,
            &format!("arn:aws:codebuild:{REGION}:{ANOTHER_ACCOUNT}:report-group/shared-group"),
            &own_root,
        );

        let mut expected: Vec<String> = projects
            .iter()
            .zip(&arns)
            .filter(|(project, _)| project.shared)
            .map(|(_, arn)| arn.clone())
            .collect();
        expected.sort();
        let body = list(&router, json!({})).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?}: expected the projects shared with this account {expected:?}, \
             got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page holds every entry, so no nextToken may be stated, got {body}"
        );

        // The mirror: the account's OWN listing reports exactly the projects it owns, by
        // name, so a project is in one listing or the other and never in both.
        let mut owned: Vec<String> = projects
            .iter()
            .filter(|project| project.owner == this)
            .map(|project| project.name.to_string())
            .collect();
        owned.sort();
        let (status, held_body) = call(&router, "ListProjects", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListProjects must succeed, got {status} with {held_body}"
        );
        assert_eq!(
            listed(&held_body, case),
            owned,
            "case {case:?}: the account's own listing reports exactly the projects it \
             owns {owned:?}, got {held_body}"
        );

        // The read is reporting only: a second call answers the same thing and the
        // store is untouched.
        let again = list(&router, json!({})).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must answer the same listing, got {again}"
        );
        let store = engine.state::<CodeBuildState>();
        let held = store.lock();
        let policies = projects
            .iter()
            .filter(|project| project.principal.is_some())
            .count()
            + 1;
        assert_eq!(
            (held.projects.len(), held.resource_policies.len()),
            (projects.len(), policies),
            "case {case:?}: a read must neither add to nor drop from the store"
        );
    }
}

/// RULE: the listing is ordered by the requested criterion in the requested direction —
/// by ARN or by when each shared project was last changed, ascending or descending —
/// and an absent member takes the ascending-by-ARN default.
#[tokio::test]
async fn the_listing_is_ordered_by_the_requested_criterion_and_direction() {
    let this = this_account();
    let own_root = format!("arn:aws:iam::{this}:root");
    // Four projects another account owns, every one of them shared with this one.
    let names = ["ord-a", "ord-b", "ord-c", "ord-d"];
    let projects: Vec<Seeded> = names
        .iter()
        .map(|name| Seeded::held(name, ANOTHER_ACCOUNT).granting(&own_root, true))
        .collect();
    let (router, engine, arns) = seed(&projects).await;

    // The instant each project was last changed at, seeded as a fixed value rather than
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
                .projects
                .get_mut(name)
                .unwrap_or_else(|| panic!("the store holds {name:?} after CreateProject"));
            stored.last_modified_millis = millis;
            stored.project.last_modified = Some(state::datetime(millis));
        }
    }

    let by_arn: Vec<String> = {
        let mut sorted = arns.clone();
        sorted.sort();
        sorted
    };
    // The seeded instants, ties broken by ARN exactly as a total order needs.
    let by_modified_time: Vec<String> = {
        let mut stamped: Vec<(i64, String)> = stamps
            .iter()
            .map(|(name, millis)| (*millis, project_arn_in(ANOTHER_ACCOUNT, name)))
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
    let projects: Vec<Seeded> = names
        .iter()
        .map(|name| Seeded::held(name, ANOTHER_ACCOUNT).granting(&own_root, true))
        .collect();
    let (router, _engine, arns) = seed(&projects).await;
    let mut expected = arns.clone();
    expected.sort();

    // The sizes span one entry per page, a size that divides the count, one that does
    // not, the exact count, and `PageSize`'s published maximum of 100 (from
    // `smithy_query(query="operation",operation="ListSharedProjects")`: @range(min: 1,
    // max: 100)), where one page holds everything.
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
            "case {page_size}: walking the pages must yield every shared project once, \
             in order, got {walked:?}"
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
        project_arn_in(ANOTHER_ACCOUNT, "never-created"),
        state::project_arn(REGION, "paged-0"),
    ] {
        let (status, body) = call(
            &router,
            "ListSharedProjects",
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
