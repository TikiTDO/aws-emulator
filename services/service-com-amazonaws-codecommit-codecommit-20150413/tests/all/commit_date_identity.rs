// SPDX-License-Identifier: CC0-1.0

//! WHICH DATE a commit carries, over the wire: awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: a commit is immutable, so the `UserInfo.date` of its author
//! and its committer is the identity date the commit object was BUILT with — "the
//! date when the specified commit was commited, in timestamp format with GMT offset"
//! (`smithy_query(query="doc",shape="UserInfo",member="date")`), git's
//! `<unix-seconds> <±hhmm>` — and every operation that reports that commit reports
//! that same date, unchanged by how often it is read and by what is written to the
//! repository afterwards. Nothing re-derives it per read.
//!
//! The oracle is git's own object naming: `commitId` is the name of the commit
//! object over the tree, the parents, the two identity lines and the message
//! ([`crate::git_object_ids`]), so recomputing it from the members a read REPORTS
//! can only reproduce the reported `commitId` when the reported dates are the ones
//! the commit was written with. A read-time clock value could not.
//!
//! The table spans what the rule generalises over rather than one recorded call:
//! the operation that minted the commit (the five that write one, including the
//! merge that writes a commit no branch names), whether the request named an
//! identity at all, the path's depth, and the reader the commit is read back
//! through (`GetCommit` and `ListFileCommitHistory`, which embeds the same commit
//! in every revision of a file).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, emulator, ok, string_at};

/// The branch every row writes on.
const BRANCH: &str = "main";

/// The branch a merging row merges from.
const FEATURE: &str = "feature";

/// The identity the seed commit of every row is written with, so the setup never
/// depends on the identity defaulting the sibling suite
/// [`crate::commit_identity_from_caller`] covers.
const SEED_NAME: &str = "Date Seeder";
const SEED_EMAIL: &str = "seed@example.com";

/// The operation a row mints its commit with: every one in this crate that writes a
/// commit object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    PutFile,
    CreateCommit,
    DeleteFile,
    /// A squash merge onto the branch: a commit written from two histories.
    SquashMerge,
    /// A merge commit no reference names, reachable only by its id.
    UnreferencedMerge,
}

impl Writer {
    fn label(self) -> &'static str {
        match self {
            Self::PutFile => "PutFile",
            Self::CreateCommit => "CreateCommit",
            Self::DeleteFile => "DeleteFile",
            Self::SquashMerge => "MergeBranchesBySquash",
            Self::UnreferencedMerge => "CreateUnreferencedMergeCommit",
        }
    }

    /// Does the row need a second branch to merge from?
    fn merges(self) -> bool {
        matches!(self, Self::SquashMerge | Self::UnreferencedMerge)
    }
}

/// One row: how the commit is minted, the file whose history is read back, and
/// whether the minting request names an identity of its own.
#[derive(Debug, Clone, Copy)]
struct Row {
    writer: Writer,
    /// The path seeded, then revised or deleted, and read back through the history.
    path: &'static str,
    /// Does the minting request carry `name`/`authorName` and `email`? Where it does
    /// not, the service fills the identity from the caller — which must not change
    /// what the DATE is.
    names_an_identity: bool,
}

/// The table. Rows differ along the operation that writes the commit, the depth of
/// the path (a root file and a nested one), and whether the write names an identity.
fn rows() -> Vec<Row> {
    vec![
        Row {
            writer: Writer::PutFile,
            path: "README.md",
            names_an_identity: true,
        },
        Row {
            writer: Writer::PutFile,
            path: "docs/sub/notes.txt",
            names_an_identity: false,
        },
        Row {
            writer: Writer::CreateCommit,
            path: "docs/sub/notes.txt",
            names_an_identity: true,
        },
        Row {
            writer: Writer::DeleteFile,
            path: "docs/sub/notes.txt",
            names_an_identity: false,
        },
        Row {
            writer: Writer::SquashMerge,
            path: "docs/sub/notes.txt",
            names_an_identity: true,
        },
        Row {
            writer: Writer::UnreferencedMerge,
            path: "README.md",
            names_an_identity: true,
        },
    ]
}

/// The git date form `UserInfo.date` carries: unix seconds, a space, then a signed
/// four-digit GMT offset. Answers the seconds, or `None` for anything else.
fn git_date_seconds(date: &str) -> Option<i64> {
    let (seconds, offset) = date.split_once(' ')?;
    let digits = offset.strip_prefix('+').or_else(|| offset.strip_suffix('-'))?;
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    seconds.parse::<i64>().ok()
}

/// A git identity line built from the members a read reported for `role`.
fn reported_identity(commit: &Value, role: &str) -> String {
    format!(
        "{} <{}> {}",
        string_at(commit, &format!("/{role}/name")),
        string_at(commit, &format!("/{role}/email")),
        string_at(commit, &format!("/{role}/date"))
    )
}

/// The git object name of the commit the REPORTED members describe — git's commit
/// encoding over the tree, the parents, the two identity lines and the message,
/// named with the shared oracle in [`crate::git_object_ids`].
fn reported_commit_object_name(commit: &Value) -> String {
    let parents: Vec<String> = commit
        .get("parents")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|parent| parent.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut payload = format!("tree {}\n", string_at(commit, "/treeId"));
    for parent in &parents {
        payload.push_str(&format!("parent {parent}\n"));
    }
    payload.push_str(&format!("author {}\n", reported_identity(commit, "author")));
    payload.push_str(&format!(
        "committer {}\n",
        reported_identity(commit, "committer")
    ));
    payload.push('\n');
    payload.push_str(commit.get("message").and_then(Value::as_str).unwrap_or(""));
    crate::git_object_ids::object_name("commit", payload.as_bytes())
}

/// The whole rule for one reported commit: the dates are in git's form, one write
/// stamped one instant on both identities, and the dates are the commit's OWN —
/// the ones its id is the name of.
fn assert_date_is_the_commits_own(commit: &Value, case: &str, reader: &str) {
    for role in ["author", "committer"] {
        let date = string_at(commit, &format!("/{role}/date"));
        assert!(
            git_date_seconds(&date).is_some(),
            "case {case:?} through {reader}: the {role} date must be \
             <unix-seconds> <±hhmm>, got {date:?}"
        );
    }
    let author = string_at(commit, "/author/date");
    let committer = string_at(commit, "/committer/date");
    assert_eq!(
        author, committer,
        "case {case:?} through {reader}: one write stamps one instant, so the author \
         and committer dates agree; got author {author:?} and committer {committer:?}"
    );
    let commit_id = string_at(commit, "/commitId");
    let recomputed = reported_commit_object_name(commit);
    assert_eq!(
        commit_id, recomputed,
        "case {case:?} through {reader}: the reported dates must be the ones the \
         commit object was built with — the git commit object name over the reported \
         members is {recomputed}, so a commit id of {commit_id} means the date \
         travelled per read, in {commit}"
    );
}

/// The commit `commit_id` as `GetCommit` reports it.
async fn fetched(router: &MultiServiceRouter, repository: &str, commit_id: &str) -> Value {
    let payload = ok(
        router,
        "GetCommit",
        &json!({ "repositoryName": repository, "commitId": commit_id }),
    )
    .await;
    payload
        .get("commit")
        .cloned()
        .unwrap_or_else(|| panic!("expected a commit in {payload}"))
}

/// The revision DAG of `path` at `specifier`.
async fn history(
    router: &MultiServiceRouter,
    repository: &str,
    path: &str,
    specifier: &str,
) -> Vec<Value> {
    let payload = ok(
        router,
        "ListFileCommitHistory",
        &json!({
            "repositoryName": repository,
            "filePath": path,
            "commitSpecifier": specifier,
        }),
    )
    .await;
    payload
        .get("revisionDag")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a revisionDag in {payload}"))
}

/// The commit a branch points at.
async fn tip_of(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    let payload = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    string_at(&payload, "/branch/commitId")
}

/// Write one file with an explicit identity, answering the commit id.
async fn write_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    path: &str,
    content: &[u8],
    message: &str,
    parent: Option<&str>,
) -> String {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": path,
        "fileContent": base64(content),
        "commitMessage": message,
        "name": SEED_NAME,
        "email": SEED_EMAIL,
    });
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    string_at(&ok(router, "PutFile", &input).await, "/commitId")
}

/// Create the row's repository, seed `row.path` on [`BRANCH`] (and a second branch
/// with a change of its own where the row merges), then mint the row's commit.
/// Answers `(the minted commit, the seed commit)`.
async fn mint(router: &MultiServiceRouter, repository: &str, row: &Row) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let seed = write_file(
        router,
        repository,
        BRANCH,
        row.path,
        b"the seeded version\n",
        "seed",
        None,
    )
    .await;

    if row.writer.merges() {
        ok(
            router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": FEATURE,
                "commitId": seed,
            }),
        )
        .await;
        write_file(
            router,
            repository,
            FEATURE,
            "feature/only.txt",
            b"a change of the other branch\n",
            "the merged change",
            Some(&seed),
        )
        .await;
    }

    let identity = |mut input: Value, name_member: &str| -> Value {
        if row.names_an_identity {
            input[name_member] = json!("Commit Writer");
            input["email"] = json!("writer@example.com");
        }
        input
    };

    let minted = match row.writer {
        Writer::PutFile => {
            let input = identity(
                json!({
                    "repositoryName": repository,
                    "branchName": BRANCH,
                    "filePath": row.path,
                    "fileContent": base64(b"a second version\n"),
                    "commitMessage": format!("revise {}", row.path),
                    "parentCommitId": seed,
                }),
                "name",
            );
            string_at(&ok(router, "PutFile", &input).await, "/commitId")
        }
        Writer::CreateCommit => {
            let input = identity(
                json!({
                    "repositoryName": repository,
                    "branchName": BRANCH,
                    "parentCommitId": seed,
                    "commitMessage": format!("commit {}", row.path),
                    "putFiles": [
                        { "filePath": row.path, "fileContent": base64(b"a committed version\n") },
                    ],
                }),
                "authorName",
            );
            string_at(&ok(router, "CreateCommit", &input).await, "/commitId")
        }
        Writer::DeleteFile => {
            let input = identity(
                json!({
                    "repositoryName": repository,
                    "branchName": BRANCH,
                    "filePath": row.path,
                    "parentCommitId": seed,
                    "commitMessage": format!("delete {}", row.path),
                }),
                "name",
            );
            string_at(&ok(router, "DeleteFile", &input).await, "/commitId")
        }
        Writer::SquashMerge => {
            let input = identity(
                json!({
                    "repositoryName": repository,
                    "sourceCommitSpecifier": FEATURE,
                    "destinationCommitSpecifier": BRANCH,
                    "targetBranch": BRANCH,
                    "commitMessage": "squash the feature branch",
                }),
                "authorName",
            );
            string_at(&ok(router, "MergeBranchesBySquash", &input).await, "/commitId")
        }
        Writer::UnreferencedMerge => {
            let input = identity(
                json!({
                    "repositoryName": repository,
                    "sourceCommitSpecifier": FEATURE,
                    "destinationCommitSpecifier": BRANCH,
                    "mergeOption": "THREE_WAY_MERGE",
                    "commitMessage": "merge without moving a branch",
                }),
                "authorName",
            );
            string_at(
                &ok(router, "CreateUnreferencedMergeCommit", &input).await,
                "/commitId",
            )
        }
    };
    (minted, seed)
}

/// Write another file on the branch: the repository moves on and the clock is read
/// again, which is what a date held by an EARLIER commit must survive.
async fn churn(router: &MultiServiceRouter, repository: &str) {
    let tip = tip_of(router, repository, BRANCH).await;
    write_file(
        router,
        repository,
        BRANCH,
        "churn.txt",
        b"a later, unrelated write\n",
        "churn",
        Some(&tip),
    )
    .await;
}

#[tokio::test]
async fn a_commits_date_is_the_one_it_was_written_with_on_every_read() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let repository = format!("commit-date-{index}");
        let case = format!("{} on {}", row.writer.label(), row.path);
        let (minted, seed) = mint(&router, &repository, &row).await;

        let first = fetched(&router, &repository, &minted).await;
        assert_date_is_the_commits_own(&first, &case, "GetCommit");
        let seeded = fetched(&router, &repository, &seed).await;
        assert_date_is_the_commits_own(&seeded, &case, "GetCommit of the seed commit");

        // A read does not change the date the next read sees.
        let again = fetched(&router, &repository, &minted).await;
        assert_eq!(
            again, first,
            "case {case:?}: every read of one commit id reports the same commit, got \
             {again} after {first}"
        );

        // Every revision of the file the history reports is the very same commit the
        // commit reader reports, down to the date.
        let dag = history(&router, &repository, row.path, &minted).await;
        assert!(
            !dag.is_empty(),
            "case {case:?}: the history of {} at {minted} holds at least the seeded \
             revision",
            row.path
        );
        for (position, entry) in dag.iter().enumerate() {
            let embedded = entry
                .get("commit")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected a commit in {entry}"));
            assert_date_is_the_commits_own(&embedded, &case, "ListFileCommitHistory");
            let id = string_at(&embedded, "/commitId");
            let read = fetched(&router, &repository, &id).await;
            assert_eq!(
                embedded, read,
                "case {case:?} position {position}: the history's commit {id} must be \
                 the commit GetCommit reports, got {embedded} against {read}"
            );
        }

        // Writing more history stamps a NEW commit with a new date; the dates already
        // handed out stay where they were.
        churn(&router, &repository).await;
        let after = fetched(&router, &repository, &minted).await;
        assert_eq!(
            after, first,
            "case {case:?}: a later write must not move the date of {minted}, got \
             {after} after {first}"
        );
        let seeded_after = fetched(&router, &repository, &seed).await;
        assert_eq!(
            seeded_after, seeded,
            "case {case:?}: a later write must not move the date of the seed commit \
             {seed}, got {seeded_after} after {seeded}"
        );
        let dag_after = history(&router, &repository, row.path, &minted).await;
        assert_eq!(
            dag_after.len(),
            dag.len(),
            "case {case:?}: the history at {minted} is the history of that commit, got \
             {} entries after {}",
            dag_after.len(),
            dag.len()
        );
        assert_eq!(
            dag_after, dag,
            "case {case:?}: the history at {minted} must repeat unchanged after a \
             later write"
        );
    }
}

#[tokio::test]
async fn dates_handed_out_never_move_as_the_history_grows() {
    let router = emulator();
    let repository = "commit-date-growing";
    let path = "docs/sub/notes.txt";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    // Rows: the history after each of five writes of the same file. Every commit's
    // date is read after it is written and after every LATER write, so a date that
    // travelled with the read — or with the repository's state — would show up.
    let mut commits: Vec<String> = Vec::new();
    let mut dates: Vec<String> = Vec::new();
    for revision in 0..5usize {
        let parent = commits.last().cloned();
        let commit_id = write_file(
            &router,
            repository,
            BRANCH,
            path,
            format!("revision {revision}\n").as_bytes(),
            &format!("write {revision}"),
            parent.as_deref(),
        )
        .await;
        let commit = fetched(&router, repository, &commit_id).await;
        assert_date_is_the_commits_own(&commit, &format!("revision {revision}"), "GetCommit");
        commits.push(commit_id);
        dates.push(string_at(&commit, "/author/date"));

        // Every commit written so far, through both readers.
        let dag = history(&router, repository, path, commits.last().expect("one write")).await;
        assert_eq!(
            dag.len(),
            commits.len(),
            "case \"after write {revision}\": every version is a revision of {path}, got \
             {} of {}",
            dag.len(),
            commits.len()
        );
        for (earlier, (id, date)) in commits.iter().zip(dates.iter()).enumerate() {
            let case = format!("commit {earlier} after write {revision}");
            let read = fetched(&router, repository, id).await;
            let actual = string_at(&read, "/author/date");
            assert_eq!(
                &actual, date,
                "case {case:?}: GetCommit must still report the date {date:?} of {id}, \
                 got {actual:?}"
            );
            let embedded = dag
                .iter()
                .filter_map(|entry| entry.get("commit"))
                .find(|commit| string_at(commit, "/commitId") == *id)
                .unwrap_or_else(|| panic!("case {case:?}: {id} is a revision of {path}"));
            let listed = string_at(embedded, "/author/date");
            assert_eq!(
                &listed, date,
                "case {case:?}: the file history must report the date {date:?} of {id}, \
                 got {listed:?}"
            );
            assert_eq!(
                string_at(embedded, "/committer/date"),
                string_at(&read, "/committer/date"),
                "case {case:?}: both readers report one committer date for {id}"
            );
        }
    }
}
