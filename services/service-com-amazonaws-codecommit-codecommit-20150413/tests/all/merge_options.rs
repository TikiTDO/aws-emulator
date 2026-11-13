// SPDX-License-Identifier: CC0-1.0

//! `GetMergeOptions` over the wire, driven as awsJson1_1 requests through the router
//! that serves the real emulator.
//!
//! The rule under test: `GetMergeOptions` reports, for the two commits the specifiers
//! resolve to in stored state, the merge base of their histories and the strategies
//! that merge admits — `FAST_FORWARD_MERGE` exactly where the destination tip is
//! already an ancestor of the source tip, and `SQUASH_MERGE` with `THREE_WAY_MERGE`
//! exactly where no path's two sides are left in conflict at the request's conflict
//! detail level and resolution strategy — and it changes nothing, so a repeat of the
//! same question answers the same way.
//!
//! The tables below span the class that rule covers — which side moved, what it
//! changed (content, mode, presence), whether the two sides touched the same file and
//! the same lines, each `ConflictDetailLevelTypeEnum` value, each
//! `ConflictResolutionStrategyTypeEnum` value, and every commit-specifier form —
//! rather than one recorded request.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::ops::types::UserInfo;
use service_com_amazonaws_codecommit_codecommit_20150413::state::merge::{
    MAX_COMMITS_WALKED_PER_REQUEST, MAX_CONTENT_BYTES_PER_REQUEST, MAX_ITEMS_TO_COMPARE_PER_REQUEST,
    MAX_MERGE_FILE_BYTES, MAX_MERGE_FILE_LINES,
};
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    Branch, FileEntry, Repository, FILE_MODE_NORMAL,
};
use service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::repository_lifecycle::{base64, emulator, emulator_with_engine, error, ok, string_at, REGION};

/// `MergeOptionTypeEnum` values the model declares
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`).
const FAST_FORWARD: &str = "FAST_FORWARD_MERGE";
const SQUASH: &str = "SQUASH_MERGE";
const THREE_WAY: &str = "THREE_WAY_MERGE";

/// The file both sides of most rows below start from, and the versions each side
/// writes: `DESTINATION_FIRST_LINE` and `SOURCE_FIRST_LINE` change the SAME line,
/// `SOURCE_LAST_LINE` a different one.
const BASE_LINES: &[u8] = b"one\ntwo\nthree\nfour\nfive\n";
const DESTINATION_FIRST_LINE: &[u8] = b"ONE\ntwo\nthree\nfour\nfive\n";
const SOURCE_FIRST_LINE: &[u8] = b"uno\ntwo\nthree\nfour\nfive\n";
const SOURCE_LAST_LINE: &[u8] = b"one\ntwo\nthree\nfour\nFIVE\n";
const BASE_KEEP: &[u8] = b"keep\n";
const DESTINATION_KEEP: &[u8] = b"keep, changed on the destination\n";
const SCRIPT: &[u8] = b"#!/bin/sh\necho hi\n";
const SCRIPT_TWO: &[u8] = b"#!/bin/sh\necho hi\necho again\n";
const DESTINATION_NEW: &[u8] = b"added on the destination\n";
const SOURCE_NEW: &[u8] = b"added on the source\n";

/// One file change a side's commit carries.
#[derive(Debug, Clone)]
enum Change<'a> {
    /// Write `content` at `path`, in the given `FileModeTypeEnum` mode (`None`
    /// leaves the NORMAL default).
    Put(&'a str, &'a [u8], Option<&'a str>),
    /// Remove `path` from the tree.
    Delete(&'a str),
}

/// A repository whose `main` (destination) and `feature` (source) branches both
/// start at one base commit and then move as a row describes.
struct Diverged {
    router: MultiServiceRouter,
    base_commit: String,
    destination_commit: String,
    source_commit: String,
}

impl Diverged {
    /// Build the repository: the base commit on `main`, `feature` branched from it,
    /// then one commit per side that has changes. A side with no changes keeps the
    /// base commit as its tip, which is how a row reaches the already-merged and
    /// nothing-to-merge shapes.
    async fn build(destination: &[Change<'_>], source: &[Change<'_>]) -> Self {
        let router = emulator();
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": REPOSITORY }),
        )
        .await;
        let base = ok(
            &router,
            "CreateCommit",
            &commit_body(
                "main",
                None,
                &[
                    Change::Put("shared.txt", BASE_LINES, None),
                    Change::Put("keep.txt", BASE_KEEP, None),
                    Change::Put("run.sh", SCRIPT, None),
                ],
            ),
        )
        .await;
        let base_commit = string_at(&base, "/commitId");
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": "feature",
                "commitId": base_commit,
            }),
        )
        .await;

        let mut destination_commit = base_commit.clone();
        if !destination.is_empty() {
            let committed = ok(
                &router,
                "CreateCommit",
                &commit_body("main", Some(&base_commit), destination),
            )
            .await;
            destination_commit = string_at(&committed, "/commitId");
        }
        let mut source_commit = base_commit.clone();
        if !source.is_empty() {
            let committed = ok(
                &router,
                "CreateCommit",
                &commit_body("feature", Some(&base_commit), source),
            )
            .await;
            source_commit = string_at(&committed, "/commitId");
        }
        Self {
            router,
            base_commit,
            destination_commit,
            source_commit,
        }
    }
}

const REPOSITORY: &str = "merges";
pub(crate) const SEEDED_REPOSITORY: &str = REPOSITORY;

/// A `CreateCommit` body carrying one side's changes.
fn commit_body(branch: &str, parent: Option<&str>, changes: &[Change<'_>]) -> Value {
    let mut puts: Vec<Value> = Vec::new();
    let mut deletes: Vec<Value> = Vec::new();
    for change in changes {
        match change {
            Change::Put(path, content, mode) => {
                let mut entry = json!({ "filePath": path, "fileContent": base64(content) });
                if let Some(mode) = mode {
                    entry["fileMode"] = json!(mode);
                }
                puts.push(entry);
            }
            Change::Delete(path) => deletes.push(json!({ "filePath": path })),
        }
    }
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "branchName": branch,
        "commitMessage": format!("{branch} changes"),
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    if !puts.is_empty() {
        body["putFiles"] = json!(puts);
    }
    if !deletes.is_empty() {
        body["deleteFiles"] = json!(deletes);
    }
    body
}

/// A `GetMergeOptions` body naming the two specifiers and only the conflict members
/// a row asks for.
fn request(source: &str, destination: &str, detail: Option<&str>, strategy: Option<&str>) -> Value {
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "sourceCommitSpecifier": source,
        "destinationCommitSpecifier": destination,
    });
    if let Some(detail) = detail {
        body["conflictDetailLevel"] = json!(detail);
    }
    if let Some(strategy) = strategy {
        body["conflictResolutionStrategy"] = json!(strategy);
    }
    body
}

fn options_of(payload: &Value) -> Vec<String> {
    payload
        .get("mergeOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a mergeOptions list in {payload}"))
        .iter()
        .map(|option| {
            option
                .as_str()
                .unwrap_or_else(|| panic!("a mergeOptions entry must be a string, got {option}"))
                .to_string()
        })
        .collect()
}

/// One row of the mergeability table.
struct Case<'a> {
    label: &'a str,
    destination: Vec<Change<'a>>,
    source: Vec<Change<'a>>,
    detail: Option<&'a str>,
    strategy: Option<&'a str>,
    expected: Vec<&'a str>,
}

#[tokio::test]
async fn merge_options_are_the_strategies_the_stored_histories_admit() {
    // Rows differ along which side moved, what it changed (content, mode, presence),
    // whether the two sides touched the same file and the same lines, and the
    // conflict detail level and resolution strategy the request names.
    let cases: Vec<Case> = vec![
        Case {
            label: "only the source moved: the destination can fast-forward to it",
            destination: vec![],
            source: vec![Change::Put("new.txt", SOURCE_NEW, None)],
            detail: None,
            strategy: None,
            expected: vec![FAST_FORWARD, SQUASH, THREE_WAY],
        },
        Case {
            label: "neither side moved: the two tips are the same commit",
            destination: vec![],
            source: vec![],
            detail: None,
            strategy: None,
            expected: vec![FAST_FORWARD, SQUASH, THREE_WAY],
        },
        Case {
            label: "only the destination moved: already merged, so no fast-forward",
            destination: vec![Change::Put("new.txt", DESTINATION_NEW, None)],
            source: vec![],
            detail: None,
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides moved, in different files",
            destination: vec![Change::Put("keep.txt", DESTINATION_KEEP, None)],
            source: vec![Change::Put("new.txt", SOURCE_NEW, None)],
            detail: None,
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides made the identical change to one file",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            detail: None,
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides changed one file, different lines, at the FILE_LEVEL default",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_LAST_LINE, None)],
            detail: None,
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "the same pair with FILE_LEVEL named explicitly",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_LAST_LINE, None)],
            detail: Some("FILE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "the same pair at LINE_LEVEL: different lines reconcile",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_LAST_LINE, None)],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "the same line changed by both sides stays a conflict at LINE_LEVEL",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "ACCEPT_SOURCE resolves a same-line conflict",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: None,
            strategy: Some("ACCEPT_SOURCE"),
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "ACCEPT_DESTINATION resolves a same-line conflict",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: None,
            strategy: Some("ACCEPT_DESTINATION"),
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "AUTOMERGE reconciles different lines even at FILE_LEVEL",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_LAST_LINE, None)],
            detail: Some("FILE_LEVEL"),
            strategy: Some("AUTOMERGE"),
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "AUTOMERGE cannot reconcile the same line",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: None,
            strategy: Some("AUTOMERGE"),
            expected: vec![],
        },
        Case {
            label: "NONE named explicitly is the default: a same-line change conflicts",
            destination: vec![Change::Put("shared.txt", DESTINATION_FIRST_LINE, None)],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: Some("LINE_LEVEL"),
            strategy: Some("NONE"),
            expected: vec![],
        },
        Case {
            label: "one side deleted the file the other changed",
            destination: vec![Change::Delete("shared.txt")],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: None,
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "a deletion against a change is no more mergeable line by line",
            destination: vec![Change::Delete("shared.txt")],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "the same deletion against a change, resolved by taking the destination",
            destination: vec![Change::Delete("shared.txt")],
            source: vec![Change::Put("shared.txt", SOURCE_FIRST_LINE, None)],
            detail: None,
            strategy: Some("ACCEPT_DESTINATION"),
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides deleted the same file",
            destination: vec![Change::Delete("keep.txt")],
            source: vec![Change::Delete("keep.txt")],
            detail: None,
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides added the same new path with different content",
            destination: vec![Change::Put("new.txt", DESTINATION_NEW, None)],
            source: vec![Change::Put("new.txt", SOURCE_NEW, None)],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "one side changed the mode, the other the content, of the same file",
            destination: vec![Change::Put("run.sh", SCRIPT, Some("EXECUTABLE"))],
            source: vec![Change::Put("run.sh", SCRIPT_TWO, None)],
            detail: None,
            strategy: None,
            expected: vec![SQUASH, THREE_WAY],
        },
        Case {
            label: "both sides set a different mode on the same file",
            destination: vec![Change::Put("run.sh", SCRIPT, Some("EXECUTABLE"))],
            source: vec![Change::Put("run.sh", SCRIPT, Some("SYMLINK"))],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
        Case {
            label: "a conflicting file beside a cleanly changed one still blocks the merge",
            destination: vec![
                Change::Put("shared.txt", DESTINATION_FIRST_LINE, None),
                Change::Put("keep.txt", DESTINATION_KEEP, None),
            ],
            source: vec![
                Change::Put("shared.txt", SOURCE_FIRST_LINE, None),
                Change::Put("new.txt", SOURCE_NEW, None),
            ],
            detail: Some("LINE_LEVEL"),
            strategy: None,
            expected: vec![],
        },
    ];

    for case in cases {
        let label = case.label;
        let diverged = Diverged::build(&case.destination, &case.source).await;
        let input = request("feature", "main", case.detail, case.strategy);
        let payload = ok(&diverged.router, "GetMergeOptions", &input).await;

        let got = options_of(&payload);
        assert_eq!(
            got, case.expected,
            "case {label:?}: expected {:?}, got {got:?} in {payload}",
            case.expected
        );
        // The merge base is the commit both branches were built from, and each tip
        // is reported as the full id the specifier resolved to.
        assert_eq!(
            string_at(&payload, "/baseCommitId"),
            diverged.base_commit,
            "case {label:?}: the merge base is the commit both branches start at"
        );
        assert_eq!(
            string_at(&payload, "/sourceCommitId"),
            diverged.source_commit,
            "case {label:?}: sourceCommitId is the tip the source specifier names"
        );
        assert_eq!(
            string_at(&payload, "/destinationCommitId"),
            diverged.destination_commit,
            "case {label:?}: destinationCommitId is the tip the destination specifier names"
        );
        // A fast-forward is offered exactly where the base IS the destination tip.
        assert_eq!(
            got.contains(&FAST_FORWARD.to_string()),
            diverged.base_commit == diverged.destination_commit,
            "case {label:?}: a fast-forward is available only where the destination tip is an \
             ancestor of the source tip, got {got:?}"
        );

        // Asking is a read: it changes nothing a later caller sees.
        let again = ok(&diverged.router, "GetMergeOptions", &input).await;
        assert_eq!(
            again, payload,
            "case {label:?}: GetMergeOptions must be repeatable"
        );
    }
}

#[tokio::test]
async fn swapping_the_two_sides_swaps_which_side_can_fast_forward() {
    // The question is directional: merging the source INTO the destination. With
    // only the source moved the destination can fast-forward; asked the other way
    // round the same pair of commits cannot.
    let diverged = Diverged::build(&[], &[Change::Put("new.txt", SOURCE_NEW, None)]).await;
    let router = &diverged.router;

    let forward = ok(router, "GetMergeOptions", &request("feature", "main", None, None)).await;
    assert_eq!(
        options_of(&forward),
        vec![FAST_FORWARD, SQUASH, THREE_WAY],
        "case \"feature into main\": the destination is behind the source, got {forward}"
    );
    let backward = ok(router, "GetMergeOptions", &request("main", "feature", None, None)).await;
    assert_eq!(
        options_of(&backward),
        vec![SQUASH, THREE_WAY],
        "case \"main into feature\": the destination is ahead of the source, got {backward}"
    );
    assert_eq!(
        string_at(&backward, "/baseCommitId"),
        diverged.base_commit,
        "case \"main into feature\": the merge base does not depend on the direction"
    );
    assert_eq!(
        string_at(&backward, "/sourceCommitId"),
        diverged.base_commit,
        "case \"main into feature\": main is still the commit both branches start at"
    );
}

#[tokio::test]
async fn every_commit_specifier_form_names_the_same_tips() {
    let diverged = Diverged::build(
        &[Change::Put("keep.txt", DESTINATION_KEEP, None)],
        &[Change::Put("new.txt", SOURCE_NEW, None)],
    )
    .await;
    let router = &diverged.router;

    // `CommitName` is "the branch, tag, HEAD, or other fully qualified reference
    // used to identify a commit": every form naming `main` (this repository's
    // default branch) must resolve to the same tip, and so to the same answer.
    let destinations = [
        "main",
        "refs/heads/main",
        "HEAD",
        diverged.destination_commit.as_str(),
    ];
    let sources = ["feature", "refs/heads/feature", diverged.source_commit.as_str()];
    for destination in destinations {
        for source in sources {
            let payload =
                ok(router, "GetMergeOptions", &request(source, destination, None, None)).await;
            let got = options_of(&payload);
            assert_eq!(
                got,
                vec![SQUASH.to_string(), THREE_WAY.to_string()],
                "case {source:?} into {destination:?}: expected a mergeable pair, got {payload}"
            );
            assert_eq!(
                string_at(&payload, "/sourceCommitId"),
                diverged.source_commit,
                "case {source:?} into {destination:?}: sourceCommitId"
            );
            assert_eq!(
                string_at(&payload, "/destinationCommitId"),
                diverged.destination_commit,
                "case {source:?} into {destination:?}: destinationCommitId"
            );
            assert_eq!(
                string_at(&payload, "/baseCommitId"),
                diverged.base_commit,
                "case {source:?} into {destination:?}: baseCommitId"
            );
        }
    }
}

#[tokio::test]
async fn a_merge_question_is_refused_for_what_it_cannot_be_asked_about() {
    let diverged = Diverged::build(
        &[Change::Put("keep.txt", DESTINATION_KEEP, None)],
        &[Change::Put("new.txt", SOURCE_NEW, None)],
    )
    .await;
    let router = &diverged.router;

    // Rows differ along WHICH part of the request cannot be answered: the
    // repository, either specifier, and each of the two conflict enums.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            json!({
                "repositoryName": "absent",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name no repository can have",
            json!({
                "repositoryName": "not a name",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "a source specifier naming nothing stored",
            request("no-such-branch", "main", None, None),
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier naming nothing stored",
            request("feature", "0123456789abcdef0123456789abcdef01234567", None, None),
            "CommitDoesNotExistException",
        ),
        (
            "a blank source specifier",
            request("   ", "main", None, None),
            "CommitRequiredException",
        ),
        (
            "a blank destination specifier",
            request("feature", "", None, None),
            "CommitRequiredException",
        ),
        (
            "a conflict detail level outside the enum",
            request("feature", "main", Some("WORD_LEVEL"), None),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a conflict resolution strategy outside the enum",
            request("feature", "main", None, Some("ACCEPT_BOTH")),
            "InvalidConflictResolutionStrategyException",
        ),
    ];

    for (label, body, expected) in cases {
        let code = error(router, "GetMergeOptions", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }
}

/// The identity every seeded commit carries. A commit's author and committer are
/// not what these suites are about; one value keeps the seeded commits readable.
fn identity() -> UserInfo {
    UserInfo {
        name: Some("tester".to_string()),
        email: Some("tester@example.com".to_string()),
        date: Some("1700000000 +0000".to_string()),
    }
}

/// Store `content` as a blob of this repository and hand back the tree entry that
/// names it.
pub(crate) fn entry(repository: &mut Repository, content: &[u8]) -> FileEntry {
    let blob_id = repository.insert_blob(content.to_vec());
    FileEntry {
        blob_id,
        file_mode: FILE_MODE_NORMAL.to_string(),
        size: i64::try_from(content.len()).unwrap_or(i64::MAX),
    }
}

/// Commit a whole tree snapshot on `branch`, moving that branch to the new
/// commit, and hand back its id.
pub(crate) fn commit_on(
    repository: &mut Repository,
    branch: &str,
    parents: Vec<String>,
    files: BTreeMap<String, FileEntry>,
) -> String {
    let (commit_id, _) = repository
        .commit_snapshot(
            files,
            parents,
            identity(),
            identity(),
            Some("a seeded commit"),
        )
        .expect("the snapshot's blobs are stored");
    repository.branches.insert(
        branch.to_string(),
        Branch {
            name: branch.to_string(),
            commit_id: commit_id.clone(),
        },
    );
    commit_id
}

/// Install a repository built by `fill` as the emulator's only repository, and
/// hand back the router that serves it.
///
/// The control plane bounds what one commit may write, so a stored state past
/// those bounds — unrelated histories, or snapshots larger than a commit may
/// carry — is reachable only by writing it here, as a push of such a history
/// would have.
pub(crate) fn seeded(fill: impl FnOnce(&mut Repository)) -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    let mut repository = Repository::new(
        REPOSITORY,
        None,
        None,
        BTreeMap::new(),
        REGION,
        SystemTime::now(),
    );
    fill(&mut repository);
    repository.default_branch = Some("main".to_string());
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .insert(REPOSITORY.to_string(), repository);
    router
}

#[tokio::test]
async fn two_tips_that_share_no_commit_have_no_merge_base() {
    // Two root commits in one repository: histories that share nothing, which the
    // control plane cannot build (every commit it makes is a child of a branch
    // tip) but a push of an unrelated history can.
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

    // Each direction, and each conflict detail level, is the same answer: with no
    // common ancestor there is nothing to evaluate either tip's changes against.
    for (source, destination) in [("feature", "main"), ("main", "feature")] {
        for detail in [None, Some("FILE_LEVEL"), Some("LINE_LEVEL")] {
            let code = error(
                &router,
                "GetMergeOptions",
                &request(source, destination, detail, None),
                400,
            )
            .await;
            assert_eq!(
                code, "TipsDivergenceExceededException",
                "case {source:?} into {destination:?} at {detail:?}: expected the tips to be \
                 unevaluable, got {code}"
            );
        }
    }
}

#[tokio::test]
async fn a_longer_history_than_one_request_walks_is_refused() {
    // A chain longer than the commits one request may visit while resolving a
    // merge base: the base resolution is charged like every other phase, so a
    // history no walk could finish within the allowance is refused rather than
    // walked to the end under the store lock.
    let chain = MAX_COMMITS_WALKED_PER_REQUEST + 1;
    let router = seeded(|repository| {
        let file = entry(repository, b"x\n");
        let files = BTreeMap::from([("base.txt".to_string(), file)]);
        let mut parents: Vec<String> = Vec::new();
        for _ in 0..chain {
            let commit = commit_on(repository, "main", parents, files.clone());
            parents = vec![commit];
        }
        // Both tips name the same long history.
        let tip = parents.first().expect("a chain of commits").clone();
        repository.branches.insert(
            "feature".to_string(),
            Branch {
                name: "feature".to_string(),
                commit_id: tip,
            },
        );
    });

    let code = error(
        &router,
        "GetMergeOptions",
        &request("feature", "main", None, None),
        400,
    )
    .await;
    assert_eq!(
        code, "MaximumItemsToCompareExceededException",
        "case \"a history of {chain} commits\": expected the walk to be refused past \
         {MAX_COMMITS_WALKED_PER_REQUEST} commits, got {code}"
    );
}

#[tokio::test]
async fn more_paths_than_one_request_compares_is_refused() {
    // Each side adds enough distinct paths that the union of the three snapshots
    // passes what one request compares, which is what bounds the comparison walk
    // whatever the stored snapshots hold.
    let per_side = MAX_ITEMS_TO_COMPARE_PER_REQUEST / 2 + 1;
    let router = seeded(|repository| {
        // One blob, named by every path: the walk is over PATHS, so the bound must
        // hold however few distinct blobs the snapshots name.
        let file = entry(repository, b"x\n");
        let base_files = BTreeMap::from([("base.txt".to_string(), file.clone())]);
        let base = commit_on(repository, "main", Vec::new(), base_files.clone());
        for (branch, prefix) in [("main", "d"), ("feature", "s")] {
            let mut files = base_files.clone();
            for ordinal in 0..per_side {
                files.insert(format!("{prefix}/{ordinal}.txt"), file.clone());
            }
            commit_on(repository, branch, vec![base.clone()], files);
        }
    });

    let code = error(
        &router,
        "GetMergeOptions",
        &request("feature", "main", None, None),
        400,
    )
    .await;
    assert_eq!(
        code, "MaximumItemsToCompareExceededException",
        "case \"a union of {} paths\": expected the comparison to be refused past \
         {MAX_ITEMS_TO_COMPARE_PER_REQUEST} items, got {code}",
        per_side * 2 + 1
    );
}

#[tokio::test]
async fn more_content_than_one_request_loads_is_refused_only_where_it_is_loaded() {
    // Two shapes of too much content, in one repository: `big.txt` is a single
    // version past what ONE merge may load, and the `many/` files are each well
    // within that ceiling but together pass what one REQUEST loads.
    let line = b"line of text that is fifty characters long, padded\n";
    let big = line.repeat(MAX_MERGE_FILE_BYTES / line.len() + 1);
    let medium = line.repeat(MAX_MERGE_FILE_BYTES / line.len() / 2);
    // Each path charges all three versions, so this many paths pass the total.
    let many = MAX_CONTENT_BYTES_PER_REQUEST / (medium.len() * 3) + 1;
    let changed = |content: &[u8], suffix: &[u8]| {
        let mut changed = Vec::from(content);
        changed.extend_from_slice(suffix);
        changed
    };
    let router = seeded(|repository| {
        let mut base_files = BTreeMap::new();
        let mut destination_files = BTreeMap::new();
        let mut source_files = BTreeMap::new();
        let mut write = |repository: &mut Repository, path: String, content: &[u8]| {
            base_files.insert(path.clone(), entry(repository, content));
            destination_files.insert(
                path.clone(),
                entry(repository, &changed(content, b"ours\n")),
            );
            source_files.insert(path, entry(repository, &changed(content, b"theirs\n")));
        };
        write(repository, "big.txt".to_string(), &big);
        for ordinal in 0..many {
            write(repository, format!("many/{ordinal}.txt"), &medium);
        }
        let base = commit_on(repository, "main", Vec::new(), base_files);
        commit_on(repository, "main", vec![base.clone()], destination_files);
        commit_on(repository, "feature", vec![base], source_files);
    });

    // FILE_LEVEL compares the stored blob ids and loads no content at all, so the
    // content allowance is not spent and the question is answered: files both
    // sides changed differently, so neither a squash nor a three-way merge.
    let payload = ok(
        &router,
        "GetMergeOptions",
        &request("feature", "main", Some("FILE_LEVEL"), None),
    )
    .await;
    let got = options_of(&payload);
    assert!(
        got.is_empty(),
        "case \"FILE_LEVEL over large content\": files both sides changed admit no merge, got \
         {got:?}"
    );

    // LINE_LEVEL and AUTOMERGE are the two ways to ask for the content itself,
    // and this much of it is past what one request loads.
    for (label, detail, strategy) in [
        ("LINE_LEVEL", Some("LINE_LEVEL"), None),
        ("AUTOMERGE", None, Some("AUTOMERGE")),
    ] {
        let code = error(
            &router,
            "GetMergeOptions",
            &request("feature", "main", detail, strategy),
            400,
        )
        .await;
        assert_eq!(
            code, "MaximumFileContentToLoadExceededException",
            "case {label:?}: expected content past the {MAX_MERGE_FILE_BYTES} bytes one merge \
             loads, or the {MAX_CONTENT_BYTES_PER_REQUEST} bytes one request loads, to be \
             refused, got {code}"
        );
    }
}

#[tokio::test]
async fn a_version_of_more_lines_than_one_merge_loads_is_refused() {
    // One file within the per-merge byte ceiling but past its line ceiling: a
    // line merge costs on the order of the product of the two sides' differing
    // lines, so the line count is bounded as well as the byte count.
    let short_lines = b"x\n".repeat(MAX_MERGE_FILE_LINES + 1);
    assert!(
        short_lines.len() < MAX_MERGE_FILE_BYTES,
        "case \"the line ceiling\": this case must be within the byte ceiling, got {} bytes",
        short_lines.len()
    );
    let router = seeded(|repository| {
        let mut ours = short_lines.clone();
        ours.extend_from_slice(b"ours\n");
        let mut theirs = short_lines.clone();
        theirs.extend_from_slice(b"theirs\n");
        let base_file = entry(repository, &short_lines);
        let destination_file = entry(repository, &ours);
        let source_file = entry(repository, &theirs);
        let base = commit_on(
            repository,
            "main",
            Vec::new(),
            BTreeMap::from([("lines.txt".to_string(), base_file)]),
        );
        commit_on(
            repository,
            "main",
            vec![base.clone()],
            BTreeMap::from([("lines.txt".to_string(), destination_file)]),
        );
        commit_on(
            repository,
            "feature",
            vec![base],
            BTreeMap::from([("lines.txt".to_string(), source_file)]),
        );
    });

    let code = error(
        &router,
        "GetMergeOptions",
        &request("feature", "main", Some("LINE_LEVEL"), None),
        400,
    )
    .await;
    assert_eq!(
        code, "MaximumFileContentToLoadExceededException",
        "case \"a version of more than {MAX_MERGE_FILE_LINES} lines\": expected it not to be \
         merged line by line, got {code}"
    );
}
