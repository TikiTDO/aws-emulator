// SPDX-License-Identifier: CC0-1.0

//! What a refusal of the commit-write and object-read paths SAYS, over the wire.
//!
//! Two rules are under test here, both about the shape of a refusal rather than
//! about which one is raised:
//!
//! 1. One error code is worded ONE way: a CONSTANT where the service's text names
//!    nothing from the request (`ParentCommitIdOutdatedException`,
//!    `ParentCommitIdRequiredException`, `NoChangeException`,
//!    `SameFileContentException`, `BlobIdDoesNotExistException`), and a TEMPLATE
//!    whose single slot is a named request value where it does — the resolved
//!    commit specifier for `CommitDoesNotExistException` raised on a member the
//!    model types `CommitName`, and the object name for the same code raised on
//!    one it types `CommitId` — whichever operation raises the code and whatever
//!    else the request said.
//! 2. Whether the body carries a `message` at all is part of the response shape,
//!    decided per operation: the branch READ (`GetBranch`) answers
//!    `BranchDoesNotExistException` with no message text, while the write paths
//!    that raise the same code carry its constant.
//!
//! A third rule rides along with the commit-specifier template, because the
//! template's slot is what it resolves: an absent `commitSpecifier` resolves to
//! `HEAD` — "If none is provided, the head commit is used"
//! (`smithy_query(query="doc",shape="GetFileInput",member="commitSpecifier")`) —
//! so a read that names no specifier follows the repository's default branch, and
//! the refusal names `HEAD`.
//!
//! Each table varies what its rule generalises over rather than one recorded
//! request: the repository and branch name (the 1- and 100-character bounds of
//! `RepositoryName`, the 1- and 256-character bounds of `BranchName`, slashes,
//! dots, underscores, digits), the file path and depth, how stale the parent id
//! is, the operation that raises the code, the form of the commit specifier
//! (absent, `HEAD`, a branch name, a fully qualified reference, a full object id)
//! and how the branch came not to exist.
//!
//! Sibling suites own the neighbouring rules: `client_error_messages.rs` pins the
//! repository-scoped codes' templates and `aws_error_message_text.rs` the
//! approval-rule and pagination codes'.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, call, emulator, git_blob_id, ok, string_at};

/// The HTTP status every error asserted here models (`errorKind: client`,
/// `httpError: 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation that
/// takes a commit or blob id gets past its own `Invalid…IdException` check and
/// reaches the rule under test. No repository holds it.
const FULL_OBJECT_ID: &str = "0123456789abcdef0123456789abcdef01234567";

/// The texts under test. Each is the service's own wording for one code, and each
/// is asserted here as a literal rather than read from the crate, so a change to
/// the crate's constant cannot change what this suite expects.
const PARENT_COMMIT_ID_OUTDATED: &str = "The parent commit ID is not valid because it is not the \
    current tip of the branch. You must provide the commit ID for the tip of the branch. The most \
    common reason for this error is that your local repository is not synchronized to the latest \
    changes in the AWS CodeCommit repository. Either synchronize your local repo by using the git \
    pull command, or use GetBranch to retrieve the latest commit ID for the branch, and then try \
    again.";
const PARENT_COMMIT_ID_REQUIRED: &str = "A parent commit ID is required. Either use GetBranch to \
    retrieve the latest commit ID for the branch, or use a Git command such as git pull or git \
    log, and then try again.";
const NO_CHANGE: &str = "Cannot create the commit. At least one change is required to create a \
    commit, but the files in the commit are exactly the same as the files in the parent commit. \
    If you want to create an empty commit, use a Git client.";
const SAME_FILE_CONTENT: &str = "The content of the file is exactly the same as the content of \
    the file in the AWS CodeCommit repository and branch you specified. The file has not been \
    added.";
const BRANCH_DOES_NOT_EXIST: &str = "The specified branch does not exist";

/// `BlobIdDoesNotExistException`'s message names the object the request asked for.
fn blob_id_does_not_exist(blob_id: &str) -> String {
    format!("AWS CodeCommit could not find the blob {blob_id} in the repository.")
}

/// `CommitDoesNotExistException`'s text for a member the model types `CommitName`
/// — a commit SPECIFIER (a branch, a tag, `HEAD` or a full commit id) — that
/// resolved to no commit. The one slot is that specifier.
fn commit_specifier_not_found(specifier: &str) -> String {
    format!(
        "AWS CodeCommit could not find the commit specifier {specifier}. Either the specifier \
         does not exist, or the commit has not yet been pushed to the remote repository."
    )
}

/// `CommitDoesNotExistException`'s text for a member the model types `CommitId` —
/// a raw git object name looked up rather than resolved. The one slot is that id.
fn commit_object_missing(commit_id: &str) -> String {
    format!("Object missing: {commit_id}")
}

/// One row of the write tables: a repository name, the branch written to and the
/// path of the file the first commit carries. The names span what
/// `RepositoryName` (len 1..100, `^[\w\.-]+$`) and `BranchName` (len 1..256)
/// allow, and the paths span root and nested entries with ASCII and non-ASCII
/// segments.
#[derive(Debug, Clone)]
struct Row {
    repository: String,
    branch: String,
    path: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            repository: "a".to_string(),
            branch: "b".to_string(),
            path: "a.txt".to_string(),
        },
        Row {
            repository: "x".repeat(100),
            branch: "z".repeat(256),
            path: "deep/nested/four/levels.txt".to_string(),
        },
        Row {
            repository: "repo-with-hyphens".to_string(),
            branch: "feature/deeply/nested-ref".to_string(),
            path: "src/main.rs".to_string(),
        },
        Row {
            repository: "repo_with_underscores.9".to_string(),
            branch: "release_1.2.3".to_string(),
            path: "docs/guía.md".to_string(),
        },
    ]
}

/// Create a repository and give it a first commit on `branch`, answering that
/// commit's id. The first write of an empty repository is what creates the branch,
/// so it carries no `parentCommitId`.
async fn seeded(router: &MultiServiceRouter, row: &Row, content: &[u8]) -> String {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": row.repository }),
    )
    .await;
    let written = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": row.repository,
            "branchName": row.branch,
            "filePath": row.path,
            "fileContent": base64(content),
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

/// Write one more commit on the branch, answering the new tip.
async fn advance(router: &MultiServiceRouter, row: &Row, parent: &str, path: &str) -> String {
    let written = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": row.repository,
            "branchName": row.branch,
            "filePath": path,
            "fileContent": base64(path.as_bytes()),
            "parentCommitId": parent,
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

/// Rule 1, `ParentCommitIdOutdatedException` — the message is the same constant
/// whichever write path raises it, however stale the named parent is, and whatever
/// repository, branch and path the request named.
#[tokio::test]
async fn parent_commit_id_outdated_message_is_a_constant() {
    let router = emulator();

    for row in rows() {
        let first = seeded(&router, &row, b"seed\n").await;
        let second = advance(&router, &row, &first, "second.txt").await;
        // Two depths of staleness: the tip's parent, and a commit two writes back.
        let third = advance(&router, &row, &second, "third.txt").await;
        assert_ne!(
            third, first,
            "case {row:?}: the three writes must leave three distinct commits"
        );

        for stale in [&first, &second] {
            let requests: Vec<(&str, Value)> = vec![
                (
                    "PutFile",
                    json!({
                        "repositoryName": row.repository,
                        "branchName": row.branch,
                        "filePath": "another.txt",
                        "fileContent": base64(b"another"),
                        "parentCommitId": stale,
                    }),
                ),
                (
                    "CreateCommit",
                    json!({
                        "repositoryName": row.repository,
                        "branchName": row.branch,
                        "parentCommitId": stale,
                        "putFiles": [
                            { "filePath": "created.txt", "fileContent": base64(b"created") },
                        ],
                    }),
                ),
                (
                    "DeleteFile",
                    json!({
                        "repositoryName": row.repository,
                        "branchName": row.branch,
                        "filePath": row.path,
                        "parentCommitId": stale,
                    }),
                ),
            ];
            for (operation, body) in requests {
                let (code, message) =
                    error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "ParentCommitIdOutdatedException",
                    "case ({operation}, {row:?}, stale {stale}): expected code \
                     ParentCommitIdOutdatedException, got {code:?}"
                );
                assert_eq!(
                    message, PARENT_COMMIT_ID_OUTDATED,
                    "case ({operation}, {row:?}, stale {stale}): expected the constant \
                     {PARENT_COMMIT_ID_OUTDATED:?}, got {message:?}"
                );
            }
        }

        // The refusals changed nothing: the branch still points at the third
        // commit, so the message was the only thing under test.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": row.repository, "branchName": row.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            third,
            "case {row:?}: a refused write must leave the tip where it was, got {branch}"
        );
    }
}

/// Rule 1, `ParentCommitIdRequiredException` — the same constant from each write
/// path that refuses a request omitting `parentCommitId` on a branch that already
/// has a tip.
#[tokio::test]
async fn parent_commit_id_required_message_is_a_constant() {
    let router = emulator();

    for row in rows() {
        seeded(&router, &row, b"seed\n").await;

        let requests: Vec<(&str, Value)> = vec![
            (
                "PutFile",
                json!({
                    "repositoryName": row.repository,
                    "branchName": row.branch,
                    "filePath": "d.txt",
                    "fileContent": base64(b"d"),
                }),
            ),
            (
                "CreateCommit",
                json!({
                    "repositoryName": row.repository,
                    "branchName": row.branch,
                    "putFiles": [{ "filePath": "e.txt", "fileContent": base64(b"e") }],
                }),
            ),
        ];
        for (operation, body) in requests {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "ParentCommitIdRequiredException",
                "case ({operation}, {row:?}): expected code ParentCommitIdRequiredException, \
                 got {code:?}"
            );
            assert_eq!(
                message, PARENT_COMMIT_ID_REQUIRED,
                "case ({operation}, {row:?}): expected the constant \
                 {PARENT_COMMIT_ID_REQUIRED:?}, got {message:?}"
            );
        }
    }
}

/// Rule 1, the no-effective-change refusals — `NoChangeException` for a commit
/// whose files reproduce the parent's tree, and its single-file sibling
/// `SameFileContentException`, each a constant that names nothing from the
/// request.
#[tokio::test]
async fn no_change_and_same_file_content_messages_are_constants() {
    let router = emulator();

    for row in rows() {
        let first = seeded(&router, &row, b"seed\n").await;
        let tip = advance(&router, &row, &first, "second.txt").await;

        // Rows: ways of asking for a commit that changes nothing — the one file
        // again, both files again, and both again with the mode they already carry
        // spelled out.
        let unchanged: Vec<(&str, Value)> = vec![
            (
                "the seeded file alone, byte for byte",
                json!([{ "filePath": row.path, "fileContent": base64(b"seed\n") }]),
            ),
            (
                "every file of the parent commit",
                json!([
                    { "filePath": row.path, "fileContent": base64(b"seed\n") },
                    { "filePath": "second.txt", "fileContent": base64(b"second.txt") },
                ]),
            ),
            (
                "every file, with the mode it already has",
                json!([
                    {
                        "filePath": row.path,
                        "fileContent": base64(b"seed\n"),
                        "fileMode": "NORMAL",
                    },
                    {
                        "filePath": "second.txt",
                        "fileContent": base64(b"second.txt"),
                        "fileMode": "NORMAL",
                    },
                ]),
            ),
        ];
        for (label, put_files) in unchanged {
            let (code, message) = error_with_message(
                &router,
                "CreateCommit",
                &json!({
                    "repositoryName": row.repository,
                    "branchName": row.branch,
                    "parentCommitId": tip,
                    "putFiles": put_files,
                }),
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "NoChangeException",
                "case ({label}, {row:?}): expected code NoChangeException, got {code:?}"
            );
            assert_eq!(
                message, NO_CHANGE,
                "case ({label}, {row:?}): expected the constant {NO_CHANGE:?}, got {message:?}"
            );
        }

        // The same non-change through the single-file path, on each file of the
        // tree, is `SameFileContentException` with its own constant.
        for (path, content) in [
            (row.path.clone(), b"seed\n".to_vec()),
            ("second.txt".to_string(), b"second.txt".to_vec()),
        ] {
            let (code, message) = error_with_message(
                &router,
                "PutFile",
                &json!({
                    "repositoryName": row.repository,
                    "branchName": row.branch,
                    "filePath": path,
                    "fileContent": base64(&content),
                    "parentCommitId": tip,
                }),
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "SameFileContentException",
                "case ({path}, {row:?}): expected code SameFileContentException, got {code:?}"
            );
            assert_eq!(
                message, SAME_FILE_CONTENT,
                "case ({path}, {row:?}): expected the constant {SAME_FILE_CONTENT:?}, \
                 got {message:?}"
            );
        }
    }
}

/// Rule 1, `BlobIdDoesNotExistException` — the message is the same constant
/// whichever repository the lookup ran in and whichever object name it was asked
/// for: a well-formed object name that no object carries states only that the blob
/// is not in the repository, naming neither the id nor the repository.
#[tokio::test]
async fn blob_id_does_not_exist_message_is_a_constant() {
    let router = emulator();

    // Two repositories, one holding a blob and one holding none, so the same
    // missing id is asked for in both: a template that named the repository would
    // differ between the rows.
    let holder = Row {
        repository: "blob-holder".to_string(),
        branch: "main".to_string(),
        path: "held.txt".to_string(),
    };
    seeded(&router, &holder, b"held\n").await;
    let empty = Row {
        repository: "blob-less".to_string(),
        branch: "main".to_string(),
        path: "unused.txt".to_string(),
    };
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": empty.repository }),
    )
    .await;

    // The blob the holder DOES hold reads back, so the rows below fail on the
    // lookup rather than on the repository.
    let held = git_blob_id(b"held\n");
    let read = ok(
        &router,
        "GetBlob",
        &json!({ "repositoryName": holder.repository, "blobId": held }),
    )
    .await;
    assert!(
        read.get("content").is_some(),
        "the blob the repository holds must read back, got {read}"
    );

    // Rows: object names no repository here holds — the content-addressed id of
    // content that was never written, an all-zero id, an all-f id, and the id of
    // content written to the OTHER repository.
    let missing: Vec<(&str, String)> = vec![
        ("a content id never written", git_blob_id(b"never written\n")),
        ("the lowest object name", "0".repeat(40)),
        ("the highest object name", "f".repeat(40)),
        ("a fixed full object id", FULL_OBJECT_ID.to_string()),
    ];
    for (label, blob_id) in missing {
        for repository in [&holder.repository, &empty.repository] {
            let (code, message) = error_with_message(
                &router,
                "GetBlob",
                &json!({ "repositoryName": repository, "blobId": blob_id }),
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "BlobIdDoesNotExistException",
                "case ({label}, {repository}): expected code BlobIdDoesNotExistException, \
                 got {code:?}"
            );
            let expected = blob_id_does_not_exist(&blob_id);
            assert_eq!(
                message, expected,
                "case ({label}, {repository}): expected message {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(repository.as_str()),
                "case ({label}, {repository}): the message must not name the repository, \
                 got {message:?}"
            );
        }
    }

    // An object name is hexadecimal, so its case does not change which object it
    // names: the same id in upper case is the same missing blob, and the same
    // sentence.
    let upper = FULL_OBJECT_ID.to_ascii_uppercase();
    let (code, message) = error_with_message(
        &router,
        "GetBlob",
        &json!({ "repositoryName": holder.repository, "blobId": upper }),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "BlobIdDoesNotExistException",
        "case (upper-case id): expected code BlobIdDoesNotExistException, got {code:?}"
    );
    assert_eq!(
        message,
        blob_id_does_not_exist(&upper),
        "case (upper-case id): expected the message to name the id the request carried, got \
         {message:?}"
    );
}

/// Rule 2 — the branch READ answers `BranchDoesNotExistException` with NO message
/// text, however the branch came not to exist, while the write paths that raise
/// the same code on the same branch carry its constant.
#[tokio::test]
async fn the_branch_read_refusal_carries_no_message() {
    let router = emulator();
    let row = Row {
        repository: "branch-read-refusal".to_string(),
        branch: "main".to_string(),
        path: "seed.txt".to_string(),
    };
    let tip = seeded(&router, &row, b"seed\n").await;

    // Rows: a branch never created, one created and then deleted, and one deleted
    // after `UpdateDefaultBranch` repointed the default away from it — the three
    // ways a branch can be absent from a repository that has others, with names
    // spanning what `BranchName` allows.
    let mut gone: Vec<(&str, String)> = vec![("never created", "n".to_string())];
    for name in [
        "side".to_string(),
        "feature/deeply/nested-ref".to_string(),
        "z".repeat(256),
    ] {
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": row.repository,
                "branchName": name,
                "commitId": tip,
            }),
        )
        .await;
        ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": row.repository, "branchName": name }),
        )
        .await;
        gone.push(("deleted by DeleteBranch", name));
    }
    // The default-branch case: a branch is made the default, the default is moved
    // off it, and then it is deleted.
    let was_default = "retired-default".to_string();
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": row.repository,
            "branchName": was_default,
            "commitId": tip,
        }),
    )
    .await;
    ok(
        &router,
        "UpdateDefaultBranch",
        &json!({
            "repositoryName": row.repository,
            "defaultBranchName": was_default,
        }),
    )
    .await;
    ok(
        &router,
        "UpdateDefaultBranch",
        &json!({ "repositoryName": row.repository, "defaultBranchName": row.branch }),
    )
    .await;
    ok(
        &router,
        "DeleteBranch",
        &json!({ "repositoryName": row.repository, "branchName": was_default }),
    )
    .await;
    gone.push(("deleted after the default was repointed", was_default));

    for (label, branch) in gone {
        let (status, payload) = call(
            &router,
            "GetBranch",
            &json!({ "repositoryName": row.repository, "branchName": branch }),
        )
        .await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case ({label}, {branch:?}): expected HTTP {CLIENT_ERROR_STATUS}, got {status} \
             with {payload}"
        );
        assert_eq!(
            payload
                .get("__type")
                .and_then(Value::as_str)
                .map(|shape| shape.rsplit('#').next().unwrap_or(shape)),
            Some("BranchDoesNotExistException"),
            "case ({label}, {branch:?}): expected BranchDoesNotExistException, got {payload}"
        );
        assert_eq!(
            payload.get("message").or_else(|| payload.get("Message")),
            None,
            "case ({label}, {branch:?}): the branch read's refusal must carry no message, \
             got {payload}"
        );

        // The same missing branch, through a write path, DOES carry the code's
        // constant: the absence of a message is this operation's response shape,
        // not a second wording of the code.
        let (code, message) = error_with_message(
            &router,
            "PutFile",
            &json!({
                "repositoryName": row.repository,
                "branchName": branch,
                "filePath": "f.txt",
                "fileContent": base64(b"f"),
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case ({label}, {branch:?}): expected BranchDoesNotExistException from PutFile, \
             got {code:?}"
        );
        assert_eq!(
            message, BRANCH_DOES_NOT_EXIST,
            "case ({label}, {branch:?}): the write path must carry the code's constant \
             {BRANCH_DOES_NOT_EXIST:?}, got {message:?}"
        );
    }
}

/// Rule 1, `CommitDoesNotExistException` — the code is worded by the KIND of
/// member that named nothing: a specifier member (`CommitName`) takes the
/// specifier template, whose one slot is the specifier the read was resolved
/// against (`HEAD` for a request that names none), and a raw object id member
/// (`CommitId`) takes the object-missing template.
#[tokio::test]
async fn commit_does_not_exist_message_names_the_resolved_specifier() {
    let router = emulator();

    // A repository with no commits at all: every read of it is refused, and a
    // request that names no specifier is refused naming HEAD.
    let unseeded = "unseeded-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": unseeded }),
    )
    .await;

    // A repository with a history, so a specifier that resolves to nothing is
    // refused for naming no commit rather than for the repository being empty.
    let row = Row {
        repository: "specifier-repo".to_string(),
        branch: "main".to_string(),
        path: "README.md".to_string(),
    };
    let first = seeded(&router, &row, b"# demo\n").await;
    advance(&router, &row, &first, "second.txt").await;

    // Rows: the specifier forms `CommitName` documents — absent (so `HEAD`),
    // `HEAD` itself, a branch name, a fully qualified reference, and a full object
    // id — each paired with the specifier the message must name, and with whether
    // the SEEDED repository resolves it (it resolves `HEAD`, so the two
    // HEAD-default rows are asked of the unseeded repository only).
    let specifiers: Vec<(&str, Option<&str>, &str, bool)> = vec![
        ("no specifier at all", None, "HEAD", false),
        ("HEAD itself", Some("HEAD"), "HEAD", false),
        ("a branch name", Some("ghost-branch"), "ghost-branch", true),
        (
            "a fully qualified reference",
            Some("refs/heads/ghost-branch"),
            "refs/heads/ghost-branch",
            true,
        ),
        (
            "a full object id",
            Some(FULL_OBJECT_ID),
            FULL_OBJECT_ID,
            true,
        ),
    ];
    for (label, sent, named, also_seeded) in specifiers {
        let mut repositories: Vec<&str> = vec![unseeded];
        if also_seeded {
            repositories.push(row.repository.as_str());
        }
        for repository in repositories {
            let requests: Vec<(&str, Value)> = vec![
                (
                    "GetFolder",
                    with_specifier(json!({ "repositoryName": repository, "folderPath": "/" }), sent),
                ),
                (
                    "GetFile",
                    with_specifier(
                        json!({ "repositoryName": repository, "filePath": "README.md" }),
                        sent,
                    ),
                ),
                (
                    "ListFileCommitHistory",
                    with_specifier(
                        json!({ "repositoryName": repository, "filePath": "README.md" }),
                        sent,
                    ),
                ),
            ];
            for (operation, body) in requests {
                let (code, message) =
                    error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "CommitDoesNotExistException",
                    "case ({operation}, {repository}, {label}): expected code \
                     CommitDoesNotExistException, got {code:?}"
                );
                let expected = commit_specifier_not_found(named);
                assert_eq!(
                    message, expected,
                    "case ({operation}, {repository}, {label}): expected message {expected:?}, \
                     got {message:?}"
                );
            }
        }
    }

    // The operations that name the commit by another member carry the template for
    // the KIND of member they name it with: a specifier member keeps the specifier
    // template, while `CreateBranch`'s `commitId` — typed `CommitId`, a raw object
    // name — takes the object-missing one.
    let others: Vec<(&str, Value, String)> = vec![
        (
            "GetDifferences",
            json!({
                "repositoryName": row.repository,
                "afterCommitSpecifier": "ghost-branch",
            }),
            commit_specifier_not_found("ghost-branch"),
        ),
        (
            "CreateBranch",
            json!({
                "repositoryName": row.repository,
                "branchName": "from-nowhere",
                "commitId": FULL_OBJECT_ID,
            }),
            commit_object_missing(FULL_OBJECT_ID),
        ),
        (
            "GetMergeOptions",
            json!({
                "repositoryName": row.repository,
                "sourceCommitSpecifier": "ghost-branch",
                "destinationCommitSpecifier": row.branch,
            }),
            commit_specifier_not_found("ghost-branch"),
        ),
    ];
    for (operation, body, expected) in others {
        let (code, message) =
            error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "CommitDoesNotExistException",
            "case {operation}: expected code CommitDoesNotExistException, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {operation}: expected message {expected:?}, got {message:?}"
        );
    }
}

/// The request body with `commitSpecifier` added when the row names one.
fn with_specifier(mut body: Value, specifier: Option<&str>) -> Value {
    if let (Some(specifier), Some(object)) = (specifier, body.as_object_mut()) {
        object.insert("commitSpecifier".to_string(), json!(specifier));
    }
    body
}

/// The HEAD default is RESOLVED, not merely named in a refusal: a read that names
/// no specifier follows the repository's default branch, and follows it again when
/// `UpdateDefaultBranch` moves it.
#[tokio::test]
async fn an_absent_commit_specifier_follows_the_default_branch() {
    let router = emulator();
    let row = Row {
        repository: "head-default-repo".to_string(),
        branch: "main".to_string(),
        path: "README.md".to_string(),
    };
    let first = seeded(&router, &row, b"# demo\n").await;
    let second = advance(&router, &row, &first, "second.txt").await;

    // A branch left at the FIRST commit, so the two defaults name different tips.
    let older = "release";
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": row.repository,
            "branchName": older,
            "commitId": first,
        }),
    )
    .await;

    // Rows: (the default branch in effect, the commit a no-specifier read must
    // resolve to).
    for (default_branch, expected_tip) in [
        (row.branch.clone(), second.clone()),
        (older.to_string(), first.clone()),
        (row.branch.clone(), second.clone()),
    ] {
        ok(
            &router,
            "UpdateDefaultBranch",
            &json!({
                "repositoryName": row.repository,
                "defaultBranchName": default_branch,
            }),
        )
        .await;
        let folder = ok(
            &router,
            "GetFolder",
            &json!({ "repositoryName": row.repository, "folderPath": "/" }),
        )
        .await;
        assert_eq!(
            string_at(&folder, "/commitId"),
            expected_tip,
            "case default {default_branch:?}: a GetFolder naming no specifier must read the \
             default branch's tip, got {folder}"
        );
        // The same answer, asked for by naming HEAD explicitly: the absent
        // specifier IS `HEAD`.
        let explicit = ok(
            &router,
            "GetFolder",
            &json!({
                "repositoryName": row.repository,
                "folderPath": "/",
                "commitSpecifier": "HEAD",
            }),
        )
        .await;
        assert_eq!(
            explicit, folder,
            "case default {default_branch:?}: naming HEAD must answer exactly what naming no \
             specifier answers, got {explicit} and {folder}"
        );
        let file = ok(
            &router,
            "GetFile",
            &json!({ "repositoryName": row.repository, "filePath": row.path }),
        )
        .await;
        assert_eq!(
            string_at(&file, "/commitId"),
            expected_tip,
            "case default {default_branch:?}: a GetFile naming no specifier must read the \
             default branch's tip, got {file}"
        );
    }
}

/// Rule 1, `DefaultBranchCannotBeDeletedException` — the template's one slot is
/// the branch the request asked to delete, as a fully qualified reference, and the
/// refusal follows the default wherever `UpdateDefaultBranch` puts it.
#[tokio::test]
async fn the_default_branch_delete_refusal_names_the_qualified_reference() {
    let router = emulator();
    let row = Row {
        repository: "default-delete-refusal".to_string(),
        branch: "main".to_string(),
        path: "seed.txt".to_string(),
    };
    let tip = seeded(&router, &row, b"seed\n").await;

    // Rows: branch names spanning what `BranchName` allows — one character, a
    // slash-separated ref name, dots and underscores, non-ASCII — each made the
    // repository's default in turn, so the slot is whichever branch the request
    // named rather than a fixed one.
    let mut default = row.branch.clone();
    for next in [
        "d".to_string(),
        "release/1.x".to_string(),
        "feature_2.0".to_string(),
        "función".to_string(),
    ] {
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": row.repository,
                "branchName": next,
                "commitId": tip,
            }),
        )
        .await;

        let (code, message) = error_with_message(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": row.repository, "branchName": default }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "DefaultBranchCannotBeDeletedException",
            "case {default:?}: expected code DefaultBranchCannotBeDeletedException, got {code:?}"
        );
        let expected = format!("The current default branch refs/heads/{default} cannot be deleted");
        assert_eq!(
            message, expected,
            "case {default:?}: expected message {expected:?}, got {message:?}"
        );

        // Once the default moves off it, the branch deletes — so the refusal was
        // about being the default and the message named the right reference.
        ok(
            &router,
            "UpdateDefaultBranch",
            &json!({
                "repositoryName": row.repository,
                "defaultBranchName": next,
            }),
        )
        .await;
        ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": row.repository, "branchName": default }),
        )
        .await;
        default = next;
    }
}
