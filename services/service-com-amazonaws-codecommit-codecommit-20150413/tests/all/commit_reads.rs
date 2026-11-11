// SPDX-License-Identifier: CC0-1.0

//! `GetCommit` over the wire, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test: `GetCommit` reports the commit that the full object id names
//! in the named repository, projected from stored state — its `commitId`, its
//! `treeId`, the full ids of the commits it was built on (`parents`, empty for a root
//! commit), its message and its author and committer — and it reports nothing else,
//! refusing a commit id that is not a full object name, one the repository does not
//! hold, or a repository that does not exist, with that case's modelled error. The
//! tables below span the class the rule covers — history depth, message presence and
//! form, author identity presence, tree shape — rather than one recorded request, and
//! the returned `commitId` is checked against the git object name recomputed here
//! from the returned fields, so the id is checked against the rule rather than
//! against itself.

use serde_json::{json, Value};
use sha1::Digest;

// The wire harness — building the emulator, signing a call, reading an answer,
// and the small value helpers — is the shared one in
// `crate::repository_lifecycle`, so no two suites can disagree about what
// calling the service means.
use crate::repository_lifecycle::{base64, emulator, error, is_object_id, ok, string_at};

/// A git identity line: `"<name> <<email>> <date>"`, with an absent name or email
/// written as empty, which is what git records for an unnamed author.
fn git_identity(user: &Value) -> String {
    let name = user.get("name").and_then(Value::as_str).unwrap_or("");
    let email = user.get("email").and_then(Value::as_str).unwrap_or("");
    let date = user
        .get("date")
        .and_then(Value::as_str)
        .unwrap_or("0 +0000");
    format!("{name} <{email}> {date}")
}

/// The object name of the commit the payload describes: git's own object name
/// function over the commit object encoding of its tree, parents, author,
/// committer and message. Computed here from the RETURNED fields, independently
/// of the service, so `commitId` is checked against the commit it claims to
/// describe.
///
/// This deliberately re-derives the encoding `crate::state::commit_id` implements
/// (`src/state.rs`, "The `commitId` of a commit"): an independent computation is
/// what makes the assertion a check of the rule rather than of the service against
/// itself. If the object-name rule ever changes, this function changes with it.
fn git_commit_id(commit: &Value) -> String {
    let mut payload = format!("tree {}\n", string_at(commit, "/treeId"));
    for parent in commit
        .get("parents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        payload.push_str(&format!(
            "parent {}\n",
            parent.as_str().expect("a parent id is a string")
        ));
    }
    payload.push_str(&format!(
        "author {}\n",
        git_identity(commit.get("author").unwrap_or(&Value::Null))
    ));
    payload.push_str(&format!(
        "committer {}\n",
        git_identity(commit.get("committer").unwrap_or(&Value::Null))
    ));
    payload.push('\n');
    if let Some(message) = commit.get("message").and_then(Value::as_str) {
        payload.push_str(message);
    }
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("commit {}\0", payload.len()).as_bytes());
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

/// One write in a row's history: one `PutFile` call, so one commit.
#[derive(Debug, Clone)]
struct Write {
    path: &'static str,
    content: Vec<u8>,
    /// The `commitMessage` as sent; `None` exercises a commit with no message.
    message: Option<String>,
    /// The author `name` as sent; `None` exercises a commit with no author name.
    name: Option<&'static str>,
    email: Option<&'static str>,
}

/// One row of the table: a repository, a branch and the history written into it.
#[derive(Debug, Clone)]
struct Row {
    repository: &'static str,
    branch: &'static str,
    writes: Vec<Write>,
}

/// The table. Rows differ along what the rule generalises over: history depth (a
/// root commit alone, two commits, four commits), message form (text, absent, 1 KiB,
/// unicode), author identity (both members, name only, neither) and tree shape (a
/// file at the root, a nested path, a second file added beside the first).
/// `RepositoryName`'s modeled `length` is 1..100 with pattern `^[\w\.-]+$`
/// (`smithy_query(query="operation",operation="GetCommit")`), which every row's
/// name satisfies.
fn rows() -> Vec<Row> {
    vec![
        Row {
            repository: "root-only",
            branch: "main",
            writes: vec![Write {
                path: "README.md",
                content: b"# root\n".to_vec(),
                message: Some("initial commit".to_string()),
                name: Some("Dev One"),
                email: Some("dev.one@example.com"),
            }],
        },
        Row {
            repository: "two.deep",
            branch: "release-1.0",
            writes: vec![
                Write {
                    path: "src/main/app.sh",
                    content: (0..=255u8).cycle().take(1024).collect(),
                    message: Some("añadir ⚙ configuración".to_string()),
                    name: Some("Dev Two"),
                    email: Some("dev.two@example.com"),
                },
                Write {
                    // No message and no author identity at all: every one of these
                    // members is optional on the write, so the read must report the
                    // commit without inventing them.
                    path: "src/main/app.sh",
                    content: b"changed\n".to_vec(),
                    message: None,
                    name: None,
                    email: None,
                },
            ],
        },
        Row {
            repository: "four_deep-history",
            branch: "b",
            writes: vec![
                Write {
                    path: "a.txt",
                    content: b"one\n".to_vec(),
                    message: Some("m".repeat(1024)),
                    name: Some("Dev Three"),
                    email: None,
                },
                Write {
                    path: "a/b/c/d/e/deep.txt",
                    content: b"two\n".to_vec(),
                    message: Some("second".to_string()),
                    name: None,
                    email: Some("dev.four@example.com"),
                },
                Write {
                    path: "a.txt",
                    content: b"three\n".to_vec(),
                    message: Some(String::new()),
                    name: Some("Dev Five"),
                    email: Some("dev.five@example.com"),
                },
                Write {
                    path: "docs/guía.md",
                    content: "⚙\n".as_bytes().to_vec(),
                    message: Some("fourth".to_string()),
                    name: Some("Dev Six"),
                    email: Some("dev.six@example.com"),
                },
            ],
        },
    ]
}

#[tokio::test]
async fn get_commit_reports_the_commit_the_object_id_names() {
    let router = emulator();
    // The principal these calls are signed by: the identity a commit records where
    // the write names no author.
    let caller = crate::wire::caller_identity_name(crate::wire::DEFAULT_ACCESS_KEY_ID);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs();

    for row in rows() {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": row.repository }),
        )
        .await;

        // Write the row's history, remembering what each commit was written from.
        let mut written: Vec<(String, Write)> = Vec::new();
        for write in &row.writes {
            let mut input = json!({
                "repositoryName": row.repository,
                "branchName": row.branch,
                "filePath": write.path,
                "fileContent": base64(&write.content),
            });
            if let Some((tip, _)) = written.last() {
                input["parentCommitId"] = json!(tip);
            }
            if let Some(message) = &write.message {
                input["commitMessage"] = json!(message);
            }
            if let Some(name) = write.name {
                input["name"] = json!(name);
            }
            if let Some(email) = write.email {
                input["email"] = json!(email);
            }
            let put = ok(&router, "PutFile", &input).await;
            written.push((string_at(&put, "/commitId"), write.clone()));
        }

        for (index, (commit_id, write)) in written.iter().enumerate() {
            let case = format!("{}#{index}", row.repository);
            let input = json!({ "repositoryName": row.repository, "commitId": commit_id });
            let payload = ok(&router, "GetCommit", &input).await;
            let commit = payload
                .get("commit")
                .unwrap_or_else(|| panic!("case {case:?}: no commit in {payload}"));

            assert_eq!(
                string_at(commit, "/commitId"),
                *commit_id,
                "case {case:?}: GetCommit must report the commit id it was asked for, got {commit}"
            );

            let tree_id = string_at(commit, "/treeId");
            assert!(
                is_object_id(&tree_id),
                "case {case:?}: expected treeId to be a 40-character object name, got {tree_id:?}"
            );

            // `parents` is the full ids of the commits this one was built on: the
            // previous commit of the branch, and nothing at all for a root commit.
            let expected_parents: Vec<String> = written
                .get(index.wrapping_sub(1))
                .filter(|_| index > 0)
                .map(|(parent, _)| vec![parent.clone()])
                .unwrap_or_default();
            let parents: Vec<String> = commit
                .get("parents")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|value| value.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_else(|| panic!("case {case:?}: no parents in {commit}"));
            assert_eq!(
                parents, expected_parents,
                "case {case:?}: expected parents {expected_parents:?}, got {parents:?}"
            );

            let message = commit.get("message").and_then(Value::as_str);
            assert_eq!(
                message,
                write.message.as_deref(),
                "case {case:?}: expected message {:?}, got {message:?}",
                write.message
            );

            for role in ["author", "committer"] {
                let user = commit
                    .get(role)
                    .unwrap_or_else(|| panic!("case {case:?}: no {role} in {commit}"));
                // "If no name is specified, the user's ARN is used as the author
                // name and committer name" — recorded as that principal's NAME —
                // and an email the write does not carry is that same name, so both
                // members are REPORTED whatever the write named.
                let expected_name = write.name.unwrap_or(caller.as_str());
                assert_eq!(
                    user.get("name").and_then(Value::as_str),
                    Some(expected_name),
                    "case {case:?}: expected {role} name {expected_name:?}, got {user}"
                );
                let expected_email = write.email.unwrap_or(caller.as_str());
                assert_eq!(
                    user.get("email").and_then(Value::as_str),
                    Some(expected_email),
                    "case {case:?}: expected {role} email {expected_email:?}, got {user}"
                );
                // `UserInfo.date` is "the date when the specified commit was
                // commited, in timestamp format with GMT offset" — the unix seconds
                // and offset git records, so it parses and is not before the test
                // started.
                let date = string_at(user, "/date");
                let (seconds, offset) = date
                    .split_once(' ')
                    .unwrap_or_else(|| panic!("case {case:?}: {role} date {date:?} has no offset"));
                let seconds: u64 = seconds
                    .parse()
                    .unwrap_or_else(|_| panic!("case {case:?}: {role} date {date:?} is not unix seconds"));
                assert!(
                    seconds >= started,
                    "case {case:?}: expected the {role} date at or after {started}, got {seconds}"
                );
                assert_eq!(
                    offset, "+0000",
                    "case {case:?}: expected a GMT offset on the {role} date, got {date:?}"
                );
            }

            // The content-addressing invariant: the id is the git object name of
            // exactly the commit that was reported.
            assert_eq!(
                git_commit_id(commit),
                *commit_id,
                "case {case:?}: commitId must be the object name of the reported commit {commit}"
            );

            // A read does not change what a later caller sees.
            let again = ok(&router, "GetCommit", &input).await;
            assert_eq!(
                again, payload,
                "case {case:?}: GetCommit must be repeatable"
            );
        }

        // The branch still points at the last commit written: nothing the reads did
        // moved it.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": row.repository, "branchName": row.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            written.last().expect("the row writes at least one commit").0,
            "repository {:?}: the branch tip must survive the reads",
            row.repository
        );
    }
}

#[tokio::test]
async fn get_commit_refuses_anything_it_cannot_name_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "guarded" }),
    )
    .await;
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "other" }),
    )
    .await;
    let put = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "guarded",
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"# guarded\n"),
            "commitMessage": "initial",
        }),
    )
    .await;
    let tip = string_at(&put, "/commitId");

    // Every row is a request GetCommit cannot answer, and the error the model
    // declares for it. The commit id cases span the ways a value can fail to be a
    // full object name (empty, a short prefix, a branch name, non-hex, too long)
    // and the ways a well-formed one can name no commit here (never written, written
    // in another repository).
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            json!({ "repositoryName": "missing-repo", "commitId": tip }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a commit of another repository",
            json!({ "repositoryName": "other", "commitId": tip }),
            "CommitIdDoesNotExistException",
        ),
        (
            "an object id no repository holds",
            json!({ "repositoryName": "guarded", "commitId": "0".repeat(40) }),
            "CommitIdDoesNotExistException",
        ),
        (
            "an empty commit id",
            json!({ "repositoryName": "guarded", "commitId": "" }),
            "CommitIdRequiredException",
        ),
        (
            "a shortened SHA id",
            json!({ "repositoryName": "guarded", "commitId": tip[..7].to_string() }),
            "InvalidCommitIdException",
        ),
        (
            "a branch name where a commit id belongs",
            json!({ "repositoryName": "guarded", "commitId": "main" }),
            "InvalidCommitIdException",
        ),
        (
            "a 40-character value that is not hexadecimal",
            json!({ "repositoryName": "guarded", "commitId": "z".repeat(40) }),
            "InvalidCommitIdException",
        ),
        (
            "an over-long object id",
            json!({ "repositoryName": "guarded", "commitId": format!("{tip}00") }),
            "InvalidCommitIdException",
        ),
        (
            "a missing commit id",
            json!({ "repositoryName": "guarded" }),
            "CommitIdRequiredException",
        ),
        (
            "a missing repository name",
            json!({ "commitId": tip }),
            "RepositoryNameRequiredException",
        ),
        (
            // `RepositoryName`'s modeled pattern is `^[\w\.-]+$`.
            "a repository name outside the modeled pattern",
            json!({ "repositoryName": "not a name", "commitId": tip }),
            "InvalidRepositoryNameException",
        ),
    ];

    for (case, input, expected) in cases {
        let code = error(&router, "GetCommit", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }

    // None of the refusals touched the store: the commit still reads back.
    let payload = ok(
        &router,
        "GetCommit",
        &json!({ "repositoryName": "guarded", "commitId": tip }),
    )
    .await;
    assert_eq!(
        string_at(&payload, "/commit/commitId"),
        tip,
        "the stored commit must still be readable after every refusal"
    );
}
