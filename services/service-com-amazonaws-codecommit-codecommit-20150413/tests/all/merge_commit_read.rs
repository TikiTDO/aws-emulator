// SPDX-License-Identifier: CC0-1.0

//! `GetMergeCommit` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: `GetMergeCommit` REPORTS a merge the service has already
//! made. It resolves each specifier to a commit in stored state, reports those two
//! commit ids and the merge base of their histories, and reports `mergedCommitId`
//! exactly when the repository holds a merge commit written for that pair of
//! commits — by `CreateUnreferencedMergeCommit`, a branch merge or a pull-request
//! merge — and leaves the member out otherwise, which includes a fast-forward
//! merge, where "there is no merge commit". It writes nothing, so the answer does
//! not depend on how often it is asked, and asking does not make a merge.
//!
//! The table below spans what the rule generalises over rather than one recorded
//! request: which operation made the merge (none, the unreferenced merge under
//! each `MergeOptionTypeEnum` option that writes a commit, both branch merges,
//! both pull-request merges, and the two fast-forward merges that write none),
//! whether the two sides conflict, the specifier form each tip is named by
//! (branch name, `refs/heads/<branch>`, full commit id, `HEAD`), and the
//! `conflictDetailLevel` / `conflictResolutionStrategy` the read names — which no
//! member of this operation's output depends on.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::merge_options::{commit_on, entry, seeded, SEEDED_REPOSITORY};
use crate::repository_lifecycle::{emulator, error, is_object_id, ok, string_at};
use crate::unreferenced_merge_commits::{commit, Change};
use crate::wire::fresh_client_request_token;

/// The repository every row below builds, and the two branches it merges: `main`
/// is the destination (it took the first commit, so it is the default branch) and
/// `feature` the source.
const REPOSITORY: &str = "merge-commit-reads";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// `MergeOptionTypeEnum` values the model declares
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`).
const THREE_WAY: &str = "THREE_WAY_MERGE";
const SQUASH: &str = "SQUASH_MERGE";

/// The file both sides start from, and the versions each side writes: the two
/// conflicting versions change the SAME line.
const BASE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";
const DESTINATION_FIRST_LINE: &str = "ONE\ntwo\nthree\nfour\nfive\n";
const SOURCE_FIRST_LINE: &str = "uno\ntwo\nthree\nfour\nfive\n";
const DESTINATION_KEEP: &str = "kept, changed on the destination\n";
const SOURCE_NEW: &str = "added on the source\n";

/// How the merge the read asks about was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Made {
    /// Nothing merged the two tips.
    Nothing,
    /// `CreateUnreferencedMergeCommit` under one merge option.
    Unreferenced(&'static str),
    /// A branch merge, named by its operation.
    Branches(&'static str),
    /// A pull-request merge, named by its operation.
    PullRequest(&'static str),
}

/// Which form the read names the two tips by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The bare branch names.
    Branches,
    /// The fully qualified reference form, `refs/heads/<branch>`.
    References,
    /// The full commit ids the two tips are at.
    CommitIds,
    /// `HEAD` for the destination (the repository's default branch), the branch
    /// name for the source.
    Head,
}

/// One row: how the merge was made, how the two sides diverged, how the read
/// names the tips, the conflict members the read carries, and whether a merge
/// commit exists for the pair that was merged.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    made: Made,
    /// Do the two sides change the same line of the same file? A conflicting pair
    /// is merged under a strategy that settles it.
    conflicting: bool,
    /// The `conflictResolutionStrategy` the MERGE was made under.
    merge_strategy: Option<&'static str>,
    /// Does the destination branch move at all? A fast-forward merge needs a
    /// destination that is still an ancestor of the source.
    destination_moves: bool,
    named: Named,
    /// The `conflictDetailLevel` the READ names.
    read_detail_level: Option<&'static str>,
    /// The `conflictResolutionStrategy` the READ names.
    read_strategy: Option<&'static str>,
    expect_merge_commit: bool,
}

impl Row {
    fn new(case: &'static str, made: Made, named: Named, expect_merge_commit: bool) -> Self {
        Self {
            case,
            made,
            conflicting: false,
            merge_strategy: None,
            destination_moves: true,
            named,
            read_detail_level: None,
            read_strategy: None,
            expect_merge_commit,
        }
    }

    fn conflicting_under(mut self, strategy: &'static str) -> Self {
        self.conflicting = true;
        self.merge_strategy = Some(strategy);
        self
    }

    fn with_a_still_destination(mut self) -> Self {
        self.destination_moves = false;
        self
    }

    fn read_with(
        mut self,
        detail_level: Option<&'static str>,
        strategy: Option<&'static str>,
    ) -> Self {
        self.read_detail_level = detail_level;
        self.read_strategy = strategy;
        self
    }
}

/// The table: rows differ along which operation made the merge, whether the two
/// sides conflict, which specifier form the read uses, and which conflict members
/// it names.
fn rows() -> Vec<Row> {
    vec![
        Row::new(
            "no merge of the two tips has been made",
            Made::Nothing,
            Named::Branches,
            false,
        ),
        Row::new(
            "no merge made, and the read names both conflict members",
            Made::Nothing,
            Named::CommitIds,
            false,
        )
        .read_with(Some("LINE_LEVEL"), Some("ACCEPT_SOURCE")),
        Row::new(
            "an unreferenced three-way merge, read by branch name",
            Made::Unreferenced(THREE_WAY),
            Named::Branches,
            true,
        ),
        Row::new(
            "an unreferenced squash merge, read by branch name",
            Made::Unreferenced(SQUASH),
            Named::Branches,
            true,
        ),
        Row::new(
            "an unreferenced three-way merge, read by qualified reference",
            Made::Unreferenced(THREE_WAY),
            Named::References,
            true,
        ),
        Row::new(
            "an unreferenced three-way merge, read by full commit id",
            Made::Unreferenced(THREE_WAY),
            Named::CommitIds,
            true,
        ),
        Row::new(
            "an unreferenced three-way merge, read with HEAD as the destination",
            Made::Unreferenced(THREE_WAY),
            Named::Head,
            true,
        ),
        Row::new(
            "an unreferenced three-way merge, read naming the documented defaults",
            Made::Unreferenced(THREE_WAY),
            Named::Branches,
            true,
        )
        .read_with(Some("FILE_LEVEL"), Some("NONE")),
        Row::new(
            "a conflicting pair merged by taking the source, read under no strategy",
            Made::Unreferenced(THREE_WAY),
            Named::Branches,
            true,
        )
        .conflicting_under("ACCEPT_SOURCE"),
        Row::new(
            "a conflicting pair merged by taking the destination, read at LINE_LEVEL",
            Made::Unreferenced(SQUASH),
            Named::CommitIds,
            true,
        )
        .conflicting_under("ACCEPT_DESTINATION")
        .read_with(Some("LINE_LEVEL"), None),
        Row::new(
            "a three-way merge of the two branches",
            Made::Branches("MergeBranchesByThreeWay"),
            Named::CommitIds,
            true,
        ),
        Row::new(
            "a squash merge of the two branches",
            Made::Branches("MergeBranchesBySquash"),
            Named::CommitIds,
            true,
        ),
        Row::new(
            "a fast-forward merge of the two branches writes no merge commit",
            Made::Branches("MergeBranchesByFastForward"),
            Named::CommitIds,
            false,
        )
        .with_a_still_destination(),
        Row::new(
            "a three-way merge of a pull request",
            Made::PullRequest("MergePullRequestByThreeWay"),
            Named::CommitIds,
            true,
        ),
        Row::new(
            "a squash merge of a pull request",
            Made::PullRequest("MergePullRequestBySquash"),
            Named::CommitIds,
            true,
        ),
        Row::new(
            "a fast-forward merge of a pull request writes no merge commit",
            Made::PullRequest("MergePullRequestByFastForward"),
            Named::CommitIds,
            false,
        )
        .with_a_still_destination(),
    ]
}

/// The repository one row works in: the base commit on `main`, `feature` branched
/// from it, each side's own commit, and the merge the row asks about.
struct Built {
    router: MultiServiceRouter,
    base: String,
    destination_tip: String,
    source_tip: String,
    /// The commit the row's merge wrote, where it wrote one.
    merge_commit: Option<String>,
}

async fn built(row: &Row) -> Built {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let base = commit(
        &router,
        REPOSITORY,
        DESTINATION,
        None,
        &[
            Change::Put("shared.txt", BASE_LINES),
            Change::Put("keep.txt", "kept\n"),
        ],
        "the base commit",
    )
    .await;
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;

    let destination_change = if row.conflicting {
        Change::Put("shared.txt", DESTINATION_FIRST_LINE)
    } else {
        Change::Put("keep.txt", DESTINATION_KEEP)
    };
    let destination_tip = if row.destination_moves {
        commit(
            &router,
            REPOSITORY,
            DESTINATION,
            Some(&base),
            &[destination_change],
            "the destination's own change",
        )
        .await
    } else {
        base.clone()
    };
    let source_change = if row.conflicting {
        Change::Put("shared.txt", SOURCE_FIRST_LINE)
    } else {
        Change::Put("new.txt", SOURCE_NEW)
    };
    let source_tip = commit(
        &router,
        REPOSITORY,
        SOURCE,
        Some(&base),
        &[source_change],
        "the source's own change",
    )
    .await;

    let merge_commit = make_the_merge(&router, row).await;
    Built {
        router,
        base,
        destination_tip,
        source_tip,
        merge_commit,
    }
}

/// Make the merge the row asks about, reporting the merge commit it wrote (the
/// fast-forward merges write none, and report the tip the reference advanced to,
/// which is not a merge commit).
async fn make_the_merge(router: &MultiServiceRouter, row: &Row) -> Option<String> {
    match row.made {
        Made::Nothing => None,
        Made::Unreferenced(merge_option) => {
            let mut input = json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
                "mergeOption": merge_option,
                "commitMessage": format!("merge for {}", row.case),
                "authorName": "Merge Tester",
                "email": "merge.tester@example.com",
            });
            if let Some(strategy) = row.merge_strategy {
                input["conflictResolutionStrategy"] = json!(strategy);
            }
            let payload = ok(router, "CreateUnreferencedMergeCommit", &input).await;
            Some(string_at(&payload, "/commitId"))
        }
        Made::Branches(operation) => {
            let mut input = json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
                "targetBranch": DESTINATION,
            });
            if operation != "MergeBranchesByFastForward" {
                input["commitMessage"] = json!(format!("merge for {}", row.case));
                input["authorName"] = json!("Merge Tester");
                input["email"] = json!("merge.tester@example.com");
                if let Some(strategy) = row.merge_strategy {
                    input["conflictResolutionStrategy"] = json!(strategy);
                }
            }
            let payload = ok(router, operation, &input).await;
            let written = string_at(&payload, "/commitId");
            (operation != "MergeBranchesByFastForward").then_some(written)
        }
        Made::PullRequest(operation) => {
            let created = ok(
                router,
                "CreatePullRequest",
                &json!({
                    "title": format!("merge for {}", row.case),
                    "targets": [{
                        "repositoryName": REPOSITORY,
                        "sourceReference": SOURCE,
                        "destinationReference": DESTINATION,
                    }],
                    "clientRequestToken": fresh_client_request_token(),
                }),
            )
            .await;
            let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
            let mut input = json!({
                "repositoryName": REPOSITORY,
                "pullRequestId": pull_request_id,
            });
            if operation != "MergePullRequestByFastForward" {
                input["commitMessage"] = json!(format!("merge for {}", row.case));
                input["authorName"] = json!("Merge Tester");
                input["email"] = json!("merge.tester@example.com");
                if let Some(strategy) = row.merge_strategy {
                    input["conflictResolutionStrategy"] = json!(strategy);
                }
            }
            let payload = ok(router, operation, &input).await;
            let written = string_at(
                &payload,
                "/pullRequest/pullRequestTargets/0/mergeMetadata/mergeCommitId",
            );
            (operation != "MergePullRequestByFastForward").then_some(written)
        }
    }
}

/// A `GetMergeCommit` body naming the two tips the row's way, carrying only the
/// conflict members the row asks for.
fn request(row: &Row, built: &Built) -> Value {
    let (source, destination) = match row.named {
        Named::Branches => (SOURCE.to_string(), DESTINATION.to_string()),
        Named::References => (
            format!("refs/heads/{SOURCE}"),
            format!("refs/heads/{DESTINATION}"),
        ),
        // The ids the two tips were at when the merge was made, which is the pair
        // a merge of those branches merged: a branch merge leaves the destination
        // branch AT its merge commit, so the branch name resolves to a different
        // commit afterwards.
        Named::CommitIds => (built.source_tip.clone(), built.destination_tip.clone()),
        Named::Head => (SOURCE.to_string(), "HEAD".to_string()),
    };
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "sourceCommitSpecifier": source,
        "destinationCommitSpecifier": destination,
    });
    if let Some(level) = row.read_detail_level {
        body["conflictDetailLevel"] = json!(level);
    }
    if let Some(strategy) = row.read_strategy {
        body["conflictResolutionStrategy"] = json!(strategy);
    }
    body
}

fn optional_at(payload: &Value, member: &str) -> Option<String> {
    payload
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_merge_commit_read_reports_the_merge_the_service_already_made() {
    for row in rows() {
        let case = row.case;
        let built = built(&row).await;
        let payload = ok(&built.router, "GetMergeCommit", &request(&row, &built)).await;

        // The two tips are reported as the full ids the specifiers resolve to in
        // stored state, and the base is the commit both branches were built from.
        assert_eq!(
            optional_at(&payload, "sourceCommitId"),
            Some(built.source_tip.clone()),
            "case {case:?}: expected the source tip {}, got {payload}",
            built.source_tip
        );
        assert_eq!(
            optional_at(&payload, "destinationCommitId"),
            Some(built.destination_tip.clone()),
            "case {case:?}: expected the destination tip {}, got {payload}",
            built.destination_tip
        );
        assert_eq!(
            optional_at(&payload, "baseCommitId"),
            Some(built.base.clone()),
            "case {case:?}: expected the merge base {}, got {payload}",
            built.base
        );

        let merged = optional_at(&payload, "mergedCommitId");
        if row.expect_merge_commit {
            let expected = built
                .merge_commit
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: the row's merge wrote no commit"));
            assert_eq!(
                merged,
                Some(expected.clone()),
                "case {case:?}: expected the merge commit {expected}, got {merged:?} in {payload}"
            );
            assert!(
                is_object_id(&expected),
                "case {case:?}: expected a git object name as the merge commit, got {expected:?}"
            );
            // The id is reported because the repository HOLDS that commit: it
            // reads back through `GetCommit`.
            let read_back = ok(
                &built.router,
                "GetCommit",
                &json!({ "repositoryName": REPOSITORY, "commitId": expected }),
            )
            .await;
            assert_eq!(
                string_at(&read_back, "/commit/commitId"),
                expected,
                "case {case:?}: the reported merge commit must resolve, got {read_back}"
            );
        } else {
            assert_eq!(
                merged, None,
                "case {case:?}: expected no mergedCommitId, got {merged:?} in {payload}"
            );
        }

        // Reporting only: the same question answers the same way, and the read
        // made no merge of a pair that had none.
        let again = ok(&built.router, "GetMergeCommit", &request(&row, &built)).await;
        assert_eq!(
            again, payload,
            "case {case:?}: a repeat of the same read must answer the same way, got {again}"
        );
    }
}

#[tokio::test]
async fn a_merge_commit_read_follows_what_the_specifiers_resolve_to() {
    // A branch merge leaves the destination branch at its merge commit, so after
    // it the branch names resolve to a DIFFERENT pair than the one that was
    // merged: the read reports the pair it resolved, and the merge commit only for
    // the pair that merge actually merged.
    let row = Row::new(
        "a three-way branch merge read by branch name afterwards",
        Made::Branches("MergeBranchesByThreeWay"),
        Named::Branches,
        true,
    );
    let built = built(&row).await;
    let merge_commit = built
        .merge_commit
        .clone()
        .expect("a three-way branch merge writes a merge commit");

    let by_name = ok(&built.router, "GetMergeCommit", &request(&row, &built)).await;
    assert_eq!(
        optional_at(&by_name, "destinationCommitId"),
        Some(merge_commit.clone()),
        "the destination branch now resolves to the merge commit {merge_commit}, got {by_name}"
    );
    assert_eq!(
        optional_at(&by_name, "mergedCommitId"),
        None,
        "nothing has merged the source tip into the merge commit, got {by_name}"
    );

    let pinned = Row {
        named: Named::CommitIds,
        ..row.clone()
    };
    let by_id = ok(&built.router, "GetMergeCommit", &request(&pinned, &built)).await;
    assert_eq!(
        optional_at(&by_id, "mergedCommitId"),
        Some(merge_commit.clone()),
        "the pair the merge merged reports its merge commit {merge_commit}, got {by_id}"
    );
}

#[tokio::test]
async fn two_tips_that_share_no_commit_report_no_merge_base() {
    // Two root commits in one repository: histories that share nothing, which the
    // control plane cannot build (every commit it makes is a child of a branch
    // tip) but a push of an unrelated history can. This operation models none of
    // the divergence or comparison-limit errors, so an unanswerable base is
    // reported as NO base rather than as a refusal.
    let router = seeded(|repository| {
        for (branch, path, content) in [
            ("main", "left.txt", &b"left\n"[..]),
            ("feature", "right.txt", &b"right\n"[..]),
        ] {
            let file = entry(repository, content);
            let files = BTreeMap::from([(path.to_string(), file)]);
            commit_on(repository, branch, Vec::new(), files);
        }
    });

    for (source, destination) in [("feature", "main"), ("main", "feature")] {
        let payload = ok(
            &router,
            "GetMergeCommit",
            &json!({
                "repositoryName": SEEDED_REPOSITORY,
                "sourceCommitSpecifier": source,
                "destinationCommitSpecifier": destination,
            }),
        )
        .await;
        assert_eq!(
            optional_at(&payload, "baseCommitId"),
            None,
            "case {source:?} into {destination:?}: two unrelated histories have no merge base, \
             got {payload}"
        );
        assert_eq!(
            optional_at(&payload, "mergedCommitId"),
            None,
            "case {source:?} into {destination:?}: nothing merged them, got {payload}"
        );
        for member in ["sourceCommitId", "destinationCommitId"] {
            let reported = optional_at(&payload, member).unwrap_or_default();
            assert!(
                is_object_id(&reported),
                "case {source:?} into {destination:?}: expected a git object name as {member}, \
                 got {reported:?} in {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_merge_commit_read_refuses_what_it_cannot_answer() {
    let row = Row::new(
        "the refusal rows' repository",
        Made::Nothing,
        Named::Branches,
        false,
    );
    let built = built(&row).await;
    let router = &built.router;

    // Rows differ along WHICH part of the request cannot be answered: the
    // repository, either specifier, and each of the two conflict enums.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            json!({
                "repositoryName": "absent",
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name no repository can have",
            json!({
                "repositoryName": "not a name",
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "a source specifier naming nothing stored",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": "no-such-branch",
                "destinationCommitSpecifier": DESTINATION,
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier naming nothing stored",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": "0123456789abcdef0123456789abcdef01234567",
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a blank source specifier",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": "   ",
                "destinationCommitSpecifier": DESTINATION,
            }),
            "CommitRequiredException",
        ),
        (
            "a blank destination specifier",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": "",
            }),
            "CommitRequiredException",
        ),
        (
            "a conflict detail level outside the enum",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
                "conflictDetailLevel": "WORD_LEVEL",
            }),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a conflict resolution strategy outside the enum",
            json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
                "conflictResolutionStrategy": "ACCEPT_BOTH",
            }),
            "InvalidConflictResolutionStrategyException",
        ),
    ];

    for (label, body, expected) in cases {
        let code = error(router, "GetMergeCommit", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }
}
