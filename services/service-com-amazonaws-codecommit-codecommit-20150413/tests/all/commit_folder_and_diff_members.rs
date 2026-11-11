// SPDX-License-Identifier: CC0-1.0

//! The members a successful commit, folder and diff read must carry.
//!
//! The rules under test, each asserted over a table that spans what it generalises
//! over rather than one request:
//!
//! 1. A commit this service writes always records an identity: "If no name is
//!    specified, the user's ARN is used as the author name and committer name"
//!    (`smithy_query(query="shape",shape="DeleteFileInput")`, the member the model
//!    states the rule on) — recorded as the NAME of that principal, the git identity
//!    field a commit carries — and an email the request does not carry is that same
//!    calling principal's name, except on the one operation whose model says its own
//!    email member "is left blank".
//!    A reader of that commit therefore reports `name`, `email` and `date` for
//!    both the author and the committer, plus `additionalData`, which is the empty
//!    string for a commit that carries no data beyond the modelled members.
//! 2. `GetFolder`'s `folderPath` is the folder that was read, named the way the
//!    tree names it — the spelling this same answer's `absolutePath` entries use —
//!    so the repository root is the empty string.
//! 3. A `DiffHunk`'s `beforeStartLine`/`afterStartLine` are 1-based line numbers
//!    ("the 1-based line number in the before blob where this hunk begins",
//!    `smithy_query(query="shape",shape="DiffHunk")`), so neither is ever 0 — not
//!    even for a hunk that covers no line of that side.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, emulator, git_blob_id, ok, string_at};
use crate::wire::{caller_identity_name, DEFAULT_ACCESS_KEY_ID};

/// The branches the merge rows work with.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// Which identity members a request carries. Rows differ along this: both of
/// them, each one alone, neither, and a name of nothing but whitespace, which
/// names nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    Both,
    NameOnly,
    EmailOnly,
    Neither,
    BlankName,
}

impl Identity {
    /// The `name`/`authorName` value this row sends, if any.
    fn sent_name(self) -> Option<&'static str> {
        match self {
            Self::Both | Self::NameOnly => Some("Ada Lovelace"),
            Self::BlankName => Some("   "),
            Self::EmailOnly | Self::Neither => None,
        }
    }

    /// The `email` value this row sends, if any.
    fn sent_email(self) -> Option<&'static str> {
        match self {
            Self::Both | Self::EmailOnly => Some("ada@example.com"),
            Self::NameOnly | Self::Neither | Self::BlankName => None,
        }
    }

    /// Add the members this row sends to a request, under the member name the
    /// operation uses for the author's name.
    fn apply(self, input: &mut Value, name_member: &str) {
        if let Some(name) = self.sent_name() {
            input[name_member] = json!(name);
        }
        if let Some(email) = self.sent_email() {
            input["email"] = json!(email);
        }
    }

    /// The name the stored commit must report: the one that was sent, or the
    /// calling principal's own name where the request named none.
    fn expected_name(self, caller: &str) -> String {
        match self.sent_name().map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => name.to_string(),
            None => caller.to_string(),
        }
    }

    /// The email the stored commit must report: the one that was sent, or the
    /// operation's own default for a request that sends none.
    fn expected_email(self, default: &str) -> String {
        match self.sent_email() {
            Some(email) => email.to_string(),
            None => default.to_string(),
        }
    }
}

/// Every identity form a row can carry.
fn identities() -> Vec<Identity> {
    vec![
        Identity::Both,
        Identity::NameOnly,
        Identity::EmailOnly,
        Identity::Neither,
        Identity::BlankName,
    ]
}

/// The operations that WRITE a commit, which the identity rule holds for. Each
/// knows the member name it takes the author's name under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    PutFile,
    CreateCommit,
    DeleteFile,
    MergeBranchesByThreeWay,
    MergeBranchesBySquash,
    CreateUnreferencedMergeCommit,
}

impl Writer {
    fn label(self) -> &'static str {
        match self {
            Self::PutFile => "PutFile",
            Self::CreateCommit => "CreateCommit",
            Self::DeleteFile => "DeleteFile",
            Self::MergeBranchesByThreeWay => "MergeBranchesByThreeWay",
            Self::MergeBranchesBySquash => "MergeBranchesBySquash",
            Self::CreateUnreferencedMergeCommit => "CreateUnreferencedMergeCommit",
        }
    }

    /// `PutFile` and `DeleteFile` take the author's name as `name`; the commit and
    /// merge operations take it as `authorName`.
    fn name_member(self) -> &'static str {
        match self {
            Self::PutFile | Self::DeleteFile => "name",
            _ => "authorName",
        }
    }

    /// The email a commit this operation writes records when the request sends
    /// none: the calling principal's name, except for the one operation whose model
    /// says its own email member "is left blank"
    /// (`smithy_query(query="shape",shape="DeleteFileInput")`).
    fn default_email(self, caller: &str) -> String {
        match self {
            Self::DeleteFile => String::new(),
            _ => caller.to_string(),
        }
    }
}

fn every_writer() -> Vec<Writer> {
    vec![
        Writer::PutFile,
        Writer::CreateCommit,
        Writer::DeleteFile,
        Writer::MergeBranchesByThreeWay,
        Writer::MergeBranchesBySquash,
        Writer::CreateUnreferencedMergeCommit,
    ]
}

/// Write one file as its own commit, with an explicit identity, so the setup of a
/// row never depends on the rule under test.
async fn seed(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    path: &str,
    content: &[u8],
    parent: Option<&str>,
) -> String {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": path,
        "fileContent": base64(content),
        "commitMessage": format!("seed {path}"),
        "name": "Seed Writer",
        "email": "seed@example.com",
    });
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    string_at(&ok(router, "PutFile", &input).await, "/commitId")
}

/// Create the row's repository with a base commit on `main` and a `feature`
/// branch holding one divergent commit, so every writer below has what it needs,
/// and answer `(destination tip, source tip)`.
async fn history(router: &MultiServiceRouter, repository: &str) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base = seed(router, repository, DESTINATION, "shared.txt", b"base\n", None).await;
    let destination = seed(
        router,
        repository,
        DESTINATION,
        "only-main.txt",
        b"main\n",
        Some(&base),
    )
    .await;
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    let source = seed(
        router,
        repository,
        SOURCE,
        "only-feature.txt",
        b"feature\n",
        Some(&base),
    )
    .await;
    (destination, source)
}

/// Drive one writer with one identity form, answering the `commitId` it wrote.
async fn write_commit(
    router: &MultiServiceRouter,
    repository: &str,
    writer: Writer,
    identity: Identity,
) -> String {
    let (destination, source) = history(router, repository).await;
    let mut input = match writer {
        Writer::PutFile => json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "filePath": "written.txt",
            "fileContent": base64(b"written\n"),
            "commitMessage": "a file written",
            "parentCommitId": destination,
        }),
        Writer::CreateCommit => json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "commitMessage": "a commit of two files",
            "parentCommitId": destination,
            "putFiles": [
                { "filePath": "one.txt", "fileContent": base64(b"one\n") },
                { "filePath": "nested/two.txt", "fileContent": base64(b"two\n") },
            ],
        }),
        Writer::DeleteFile => json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "filePath": "only-main.txt",
            "parentCommitId": destination,
            "commitMessage": "a file deleted",
        }),
        Writer::MergeBranchesByThreeWay | Writer::MergeBranchesBySquash => json!({
            "repositoryName": repository,
            "sourceCommitSpecifier": source,
            "destinationCommitSpecifier": destination,
            "targetBranch": DESTINATION,
            "commitMessage": "a merge of two branches",
        }),
        Writer::CreateUnreferencedMergeCommit => json!({
            "repositoryName": repository,
            "sourceCommitSpecifier": source,
            "destinationCommitSpecifier": destination,
            "mergeOption": "THREE_WAY_MERGE",
            "commitMessage": "an unreferenced merge",
        }),
    };
    identity.apply(&mut input, writer.name_member());
    let payload = ok(router, writer.label(), &input).await;
    string_at(&payload, "/commitId")
}

/// Assert the commit `payload` describes carries every modelled member of the
/// identity rule, with the values `identity` asks for.
fn assert_commit_members(
    commit: &Value,
    case: &str,
    identity: Identity,
    caller: &str,
    default_email: &str,
) {
    for role in ["author", "committer"] {
        let user = commit
            .get(role)
            .unwrap_or_else(|| panic!("case {case}: expected a {role} in {commit}"));
        let expected_name = identity.expected_name(caller);
        assert_eq!(
            user.get("name").and_then(Value::as_str),
            Some(expected_name.as_str()),
            "case {case}: expected {role} name {expected_name:?}, got {user}"
        );
        let expected_email = identity.expected_email(default_email);
        assert_eq!(
            user.get("email").and_then(Value::as_str),
            Some(expected_email.as_str()),
            "case {case}: expected {role} email {expected_email:?}, got {user}"
        );
        // `UserInfo.date` is "the date when the specified commit was commited, in
        // timestamp format with GMT offset": unix seconds and the offset git
        // records.
        let date = user
            .get("date")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a {role} date in {commit}"));
        let (seconds, offset) = date
            .split_once(' ')
            .unwrap_or_else(|| panic!("case {case}: expected a GMT offset in {date:?}"));
        assert!(
            seconds.parse::<u64>().is_ok(),
            "case {case}: expected unix seconds in the {role} date, got {date:?}"
        );
        assert_eq!(
            offset, "+0000",
            "case {case}: expected a GMT offset on the {role} date, got {date:?}"
        );
    }
    // "Any other data associated with the specified commit": a commit this service
    // stores carries none, which is the EMPTY string rather than a missing member.
    assert_eq!(
        commit.get("additionalData").and_then(Value::as_str),
        Some(""),
        "case {case}: expected additionalData to be present and empty, got {commit}"
    );
}

#[tokio::test]
async fn every_written_commit_reads_back_with_a_full_identity() {
    let router = emulator();
    let caller = caller_identity_name(DEFAULT_ACCESS_KEY_ID);

    for (index, writer) in every_writer().into_iter().enumerate() {
        for (variant, identity) in identities().into_iter().enumerate() {
            let case = format!("{} with {identity:?}", writer.label());
            let repository = format!("identity-{index}-{variant}");
            let commit_id = write_commit(&router, &repository, writer, identity).await;

            let payload = ok(
                &router,
                "GetCommit",
                &json!({ "repositoryName": repository, "commitId": commit_id }),
            )
            .await;
            let commit = payload
                .get("commit")
                .unwrap_or_else(|| panic!("case {case}: expected a commit in {payload}"));
            assert_eq!(
                commit.get("commitId").and_then(Value::as_str),
                Some(commit_id.as_str()),
                "case {case}: GetCommit must report the commit it was asked for, got {commit}"
            );
            assert_commit_members(
                commit,
                &case,
                identity,
                &caller,
                &writer.default_email(&caller),
            );
        }
    }
}

#[tokio::test]
async fn the_revision_dag_embeds_the_same_full_commit() {
    let router = emulator();
    let caller = caller_identity_name(DEFAULT_ACCESS_KEY_ID);
    let repository = "history-identity";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    // One history whose revisions carry a different identity form each: the DAG
    // then reports every form in one answer, newest first.
    let mut parent: Option<String> = None;
    let mut written: Vec<(String, Identity)> = Vec::new();
    for (revision, identity) in identities().into_iter().enumerate() {
        let mut input = json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "filePath": "tracked.txt",
            "fileContent": base64(format!("revision {revision}\n").as_bytes()),
            "commitMessage": format!("revision {revision}"),
        });
        if let Some(parent) = &parent {
            input["parentCommitId"] = json!(parent);
        }
        identity.apply(&mut input, "name");
        let commit_id = string_at(&ok(&router, "PutFile", &input).await, "/commitId");
        parent = Some(commit_id.clone());
        written.push((commit_id, identity));
    }

    let listed = ok(
        &router,
        "ListFileCommitHistory",
        &json!({ "repositoryName": repository, "filePath": "tracked.txt" }),
    )
    .await;
    let dag = listed
        .get("revisionDag")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a revisionDag in {listed}"));
    assert_eq!(
        dag.len(),
        written.len(),
        "the DAG must name every revision of the file, got {listed}"
    );

    for (position, entry) in dag.iter().enumerate() {
        let (commit_id, identity) = &written[written.len() - 1 - position];
        let case = format!("revisionDag[{position}] with {identity:?}");
        let commit = entry
            .get("commit")
            .unwrap_or_else(|| panic!("case {case}: expected a commit in {entry}"));
        assert_eq!(
            commit.get("commitId").and_then(Value::as_str),
            Some(commit_id.as_str()),
            "case {case}: expected the embedded commit {commit_id}, got {entry}"
        );
        assert_commit_members(
            commit,
            &case,
            *identity,
            &caller,
            &Writer::PutFile.default_email(&caller),
        );
    }
}

/// One file of the tree the folder rows read.
struct Entry {
    path: &'static str,
    content: &'static [u8],
}

/// The tree the folder rows read: files at the root and at three depths, so the
/// rows span the root, a folder holding files, and a nested folder.
fn folder_tree() -> Vec<Entry> {
    vec![
        Entry {
            path: "README.md",
            content: b"# root\n",
        },
        Entry {
            path: "src/main.rs",
            content: b"fn main() {}\n",
        },
        Entry {
            path: "src/util/helper.rs",
            content: b"pub fn help() {}\n",
        },
        Entry {
            path: "docs/guide/intro.md",
            content: b"intro\n",
        },
    ]
}

/// The canonical name of the folder a `folderPath` addresses: no leading or
/// trailing separator, and the root named by no segment at all.
fn canonical(folder_path: &str) -> String {
    folder_path.trim_matches('/').to_string()
}

#[tokio::test]
async fn get_folder_echoes_the_canonical_path_of_the_folder_it_read() {
    let router = emulator();
    let repository = "folder-paths";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let mut parent: Option<String> = None;
    for entry in folder_tree() {
        parent = Some(
            seed(
                &router,
                repository,
                DESTINATION,
                entry.path,
                entry.content,
                parent.as_deref(),
            )
            .await,
        );
    }

    // Rows: the root in both spellings it is addressed by, a first-level folder
    // with and without separators around it, and folders at depth two and three.
    let rows = [
        "",
        "/",
        "src",
        "/src",
        "src/",
        "/src/",
        "docs",
        "src/util",
        "/docs/guide",
        "docs/guide/",
    ];
    for folder_path in rows {
        let case = format!("folderPath {folder_path:?}");
        let payload = ok(
            &router,
            "GetFolder",
            &json!({ "repositoryName": repository, "folderPath": folder_path }),
        )
        .await;
        let expected = canonical(folder_path);
        let reported = string_at(&payload, "/folderPath");
        assert_eq!(
            reported, expected,
            "case {case}: expected folderPath {expected:?}, got {reported:?}"
        );

        // And the echoed path is the one the answer's own entries agree with:
        // joining it to a child's relative path gives that child's absolute path,
        // so no reader builds a path the entries do not report.
        for member in ["files", "subFolders", "symbolicLinks"] {
            for child in payload
                .get(member)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let relative = string_at(&child, "/relativePath");
                let absolute = string_at(&child, "/absolutePath");
                let joined = if reported.is_empty() {
                    relative.clone()
                } else {
                    format!("{reported}/{relative}")
                };
                assert_eq!(
                    joined, absolute,
                    "case {case}: {member} entry {relative:?} must sit at {joined:?} under the \
                     reported folderPath, got {absolute:?}"
                );
            }
        }
    }
}

/// One row of the hunk table: two versions of a file and, for the hunk the
/// difference makes with no context at all, the line count each side holds.
struct Hunks {
    label: &'static str,
    before: &'static str,
    after: &'static str,
}

/// The table. Rows differ along what the rule generalises over: a pure insertion
/// at the start of the before blob, into an EMPTY before blob, in the middle and
/// at the end, a pure deletion at the start of the after blob and of the whole
/// file, and a replacement, which holds lines on both sides.
fn hunk_rows() -> Vec<Hunks> {
    vec![
        Hunks {
            label: "a line inserted before the first",
            before: "beta\n",
            after: "alpha\nbeta\n",
        },
        Hunks {
            label: "a whole file added against an empty before blob",
            before: "",
            after: "one\ntwo\nthree\n",
        },
        Hunks {
            label: "a run inserted in the middle",
            before: "alpha\nomega\n",
            after: "alpha\none\ntwo\nomega\n",
        },
        Hunks {
            label: "a line appended at the end",
            before: "alpha\nbeta\n",
            after: "alpha\nbeta\ngamma\n",
        },
        Hunks {
            label: "the first line deleted",
            before: "alpha\nbeta\n",
            after: "beta\n",
        },
        Hunks {
            label: "every line deleted",
            before: "alpha\nbeta\n",
            after: "\n",
        },
        Hunks {
            label: "a replaced line",
            before: "alpha\nbeta\ngamma\n",
            after: "alpha\nBETA\ngamma\n",
        },
    ]
}

/// Store two versions of a file in `repository` and answer their `(before, after)`
/// blob ids.
///
/// An EMPTY before version is not written at all: `fileContent` is required to
/// carry bytes, so the empty before-state is the one a request asks for by
/// leaving `beforeBlobId` out, and the id answered for it is the one the row does
/// not send.
async fn blob_pair(
    router: &MultiServiceRouter,
    repository: &str,
    before: &[u8],
    after: &[u8],
) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let mut parent: Option<String> = None;
    if !before.is_empty() {
        parent = Some(seed(router, repository, DESTINATION, "diff.txt", before, None).await);
    }
    seed(
        router,
        repository,
        DESTINATION,
        "diff.txt",
        after,
        parent.as_deref(),
    )
    .await;
    (git_blob_id(before), git_blob_id(after))
}

#[tokio::test]
async fn a_hunk_never_begins_at_line_zero() {
    let router = emulator();

    for (index, row) in hunk_rows().iter().enumerate() {
        let label = row.label;
        let repository = format!("hunk-start-{index}");
        let (before_id, after_id) = blob_pair(
            &router,
            &repository,
            row.before.as_bytes(),
            row.after.as_bytes(),
        )
        .await;

        // Every context width the member accepts, so the assertion holds for the
        // hunk with no context — where a side can hold no line at all — as well as
        // for the widened ones.
        for context in [0, 1, 3, 20] {
            let mut input = json!({
                "repositoryName": repository,
                "afterBlobId": after_id,
                "contextLines": context,
            });
            // The first row's before blob is empty, which no stored blob of a file
            // written with content is: that case is asked for by leaving
            // `beforeBlobId` out, "a diff against an empty before-state".
            if !row.before.is_empty() {
                input["beforeBlobId"] = json!(before_id);
            }
            let payload = ok(&router, "GetBlobDifferences", &input).await;
            let hunks = payload
                .get("hunks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("case {label:?}: expected hunks in {payload}"));
            assert!(
                !hunks.is_empty(),
                "case {label:?} at context {context}: the two versions differ, so there is a \
                 hunk, got {payload}"
            );

            for (position, hunk) in hunks.iter().enumerate() {
                let case = format!("{label:?} at context {context} hunk {position}");
                for (start, count) in [
                    ("beforeStartLine", "beforeLineCount"),
                    ("afterStartLine", "afterLineCount"),
                ] {
                    let reported = hunk
                        .get(start)
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| panic!("case {case}: expected {start} in {hunk}"));
                    assert!(
                        reported >= 1,
                        "case {case}: {start} is a 1-based line number, got {reported} in {hunk}"
                    );
                    let lines = hunk
                        .get(count)
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| panic!("case {case}: expected {count} in {hunk}"));
                    assert!(
                        lines >= 0,
                        "case {case}: {count} cannot be negative, got {lines} in {hunk}"
                    );
                }
            }

            // The changed lines of the hunks are numbered from 1 on each side too,
            // so nothing in the answer reports line 0.
            let numbers: BTreeSet<i64> = hunks
                .iter()
                .flat_map(|hunk| {
                    hunk.get("changes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .flat_map(|change| {
                    ["beforeLineNumber", "afterLineNumber"]
                        .iter()
                        .filter_map(|member| change.get(*member).and_then(Value::as_i64))
                        .collect::<Vec<i64>>()
                })
                .collect();
            assert!(
                numbers.iter().all(|line| *line >= 1),
                "case {label:?} at context {context}: every reported line number is 1-based, \
                 got {numbers:?}"
            );
        }
    }
}
