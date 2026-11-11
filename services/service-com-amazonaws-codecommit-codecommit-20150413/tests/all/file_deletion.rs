// SPDX-License-Identifier: CC0-1.0

//! `DeleteFile` over the wire, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test: `DeleteFile` removes the entry its `filePath` names from the
//! tree the named branch's HEAD commit (`parentCommitId`) holds, writes that tree as a
//! NEW commit on the branch with the caller's author identity and message, and reports
//! the removed blob's id, the new commit and tree ids and the path it deleted — the
//! file staying readable in every earlier commit, the folders the deletion emptied
//! kept (with a `.gitkeep` placeholder) only when `keepEmptyFolders` is true, and every
//! request that cannot be applied refused with that case's modelled error, persisting
//! nothing.
//!
//! The tables below span what the rule generalises over rather than the one recorded
//! request: where the deleted file lives (repository root, one folder, a four-level
//! chain), whether its folder still holds another entry, how the path is spelled
//! (bare, leading separator, non-ASCII), `keepEmptyFolders` (absent, false, true), the
//! size of the tree left behind (several files down to none at all) and which part of
//! a request is at fault. The `blobId` is compared with git's own object name function
//! recomputed in this file, and the `treeId` with the one a second repository holding
//! the same remaining content reports, so both are checked against the rule rather
//! than against the service itself.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness — building the emulator, signing a call, reading an answer, and
// the small value helpers — is the shared one in `crate::repository_lifecycle`, so
// no two suites can disagree about what calling the service means.
use crate::repository_lifecycle::{
    base64, emulator, emulator_with_engine, error, from_base64, git_blob_id, is_object_id, ok,
    string_at,
};

/// The file a kept empty folder holds. CodeCommit documents the name on
/// `CreateCommit`'s `keepEmptyFolders`: "If true, a .gitkeep file is created for
/// empty folders."
const PLACEHOLDER: &str = ".gitkeep";

/// One file written into a case's repository, as its own commit.
#[derive(Debug, Clone)]
struct Written {
    /// The path as SENT; a leading separator addresses the same tree entry as none.
    path: &'static str,
    content: &'static [u8],
}

/// One row of the deletion table: a repository holding `files`, from which
/// `sent_path` is deleted.
#[derive(Debug, Clone)]
struct Case {
    repository: &'static str,
    branch: &'static str,
    files: Vec<Written>,
    /// The `filePath` the `DeleteFile` request carries.
    sent_path: &'static str,
    /// `keepEmptyFolders` as sent; `None` omits the member, whose modelled default
    /// is false (`smithy_query(query="operation",operation="DeleteFile")`).
    keep_empty_folders: Option<bool>,
}

impl Case {
    /// The path the service addresses the deleted file by: every path in a git tree
    /// is relative to the tree's root.
    fn deleted(&self) -> String {
        canonical(self.sent_path)
    }

    fn keeps(&self) -> bool {
        self.keep_empty_folders.unwrap_or(false)
    }

    /// The content the deleted file was written with.
    fn deleted_content(&self) -> &'static [u8] {
        self.files
            .iter()
            .find(|file| canonical(file.path) == self.deleted())
            .map(|file| file.content)
            .unwrap_or_else(|| panic!("case {}: the table deletes a file it wrote", self.repository))
    }
}

fn canonical(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

/// Every folder a path lives under, from its immediate parent outwards:
/// `"a/b/c.txt"` lives under `"a/b"` and then `"a"`. A path at the root of the tree
/// lives under no folder, so a deletion there can empty none.
fn ancestor_folders(path: &str) -> Vec<&str> {
    let mut folders = Vec::new();
    let mut rest = path;
    while let Some((folder, _)) = rest.rsplit_once('/') {
        folders.push(folder);
        rest = folder;
    }
    folders
}

/// The table. Rows differ along where the deleted file lives, whether its folder is
/// emptied by the deletion, how its path is spelled, what `keepEmptyFolders` carries
/// and how much tree is left behind.
fn cases() -> Vec<Case> {
    vec![
        // A file at the root, beside a file in a folder; the member is omitted
        // altogether, so the default (false) applies and no folder is emptied.
        Case {
            repository: "root-file",
            branch: "main",
            files: vec![
                Written {
                    path: "README.md",
                    content: b"# demo\n",
                },
                Written {
                    path: "src/main.rs",
                    content: b"fn main() {}\n",
                },
            ],
            sent_path: "README.md",
            keep_empty_folders: None,
        },
        // A file in a folder that still holds another file afterwards: the folder
        // survives whatever `keepEmptyFolders` says.
        Case {
            repository: "folder-keeps-sibling",
            branch: "trunk",
            files: vec![
                Written {
                    path: "src/main.rs",
                    content: b"fn main() {}\n",
                },
                Written {
                    path: "src/lib.rs",
                    content: b"pub fn run() {}\n",
                },
            ],
            sent_path: "src/main.rs",
            keep_empty_folders: Some(false),
        },
        Case {
            repository: "folder-keeps-sibling-kept",
            branch: "trunk",
            files: vec![
                Written {
                    path: "src/main.rs",
                    content: b"fn main() {}\n",
                },
                Written {
                    path: "src/lib.rs",
                    content: b"pub fn run() {}\n",
                },
            ],
            sent_path: "src/lib.rs",
            keep_empty_folders: Some(true),
        },
        // The only file of a folder, with the folder structure NOT kept: the folder
        // goes with the file, as a git tree holds no empty subtree.
        Case {
            repository: "folder-emptied",
            branch: "main",
            files: vec![
                Written {
                    path: "README.md",
                    content: b"# demo\n",
                },
                Written {
                    path: "src/only.rs",
                    content: b"fn only() {}\n",
                },
            ],
            sent_path: "src/only.rs",
            keep_empty_folders: Some(false),
        },
        // The same deletion with the structure KEPT: the folder stays, holding the
        // placeholder.
        Case {
            repository: "folder-kept",
            branch: "main",
            files: vec![
                Written {
                    path: "README.md",
                    content: b"# demo\n",
                },
                Written {
                    path: "src/only.rs",
                    content: b"fn only() {}\n",
                },
            ],
            sent_path: "src/only.rs",
            keep_empty_folders: Some(true),
        },
        // A four-level chain emptied by one deletion, kept: ONE placeholder in the
        // deepest folder keeps every folder above it non-empty.
        Case {
            repository: "deep-chain-kept",
            branch: "release-1.0",
            files: vec![
                Written {
                    path: "a/b/c/d/deep.txt",
                    content: b"deep",
                },
                Written {
                    path: "top.txt",
                    content: b"top",
                },
            ],
            sent_path: "a/b/c/d/deep.txt",
            keep_empty_folders: Some(true),
        },
        // The same chain, not kept: every folder of the chain goes.
        Case {
            repository: "deep-chain-dropped",
            branch: "release-1.0",
            files: vec![
                Written {
                    path: "a/b/c/d/deep.txt",
                    content: b"deep",
                },
                Written {
                    path: "top.txt",
                    content: b"top",
                },
            ],
            sent_path: "a/b/c/d/deep.txt",
            keep_empty_folders: None,
        },
        // A non-ASCII path, sent with a leading separator: the same entry as without
        // one. Its folder keeps another file, so nothing is emptied.
        Case {
            repository: "unicode-path",
            branch: "feature/docs",
            files: vec![
                Written {
                    path: "docs/gu\u{ed}a.md",
                    content: "\u{2699} configuraci\u{f3}n\n".as_bytes(),
                },
                Written {
                    path: "docs/readme.md",
                    content: b"docs\n",
                },
            ],
            sent_path: "/docs/gu\u{ed}a.md",
            keep_empty_folders: Some(false),
        },
        // The only file in the whole repository: the tree left behind is empty and
        // the branch still points at the deletion commit.
        Case {
            repository: "last-file",
            branch: "main",
            files: vec![Written {
                path: "only.txt",
                content: b"only\n",
            }],
            sent_path: "/only.txt",
            keep_empty_folders: Some(false),
        },
        // A file at the root with the structure kept: the root is the tree itself,
        // not a folder a deletion can empty, so no placeholder is written.
        Case {
            repository: "root-file-kept",
            branch: "main",
            files: vec![
                Written {
                    path: "drop.txt",
                    content: b"drop\n",
                },
                Written {
                    path: "stay.txt",
                    content: b"stay\n",
                },
            ],
            sent_path: "drop.txt",
            keep_empty_folders: Some(true),
        },
    ]
}

/// Create `repository` and write every file into it, one `PutFile` commit each.
/// Returns the tip the last write left, which is the HEAD a deletion has to name.
async fn write_files(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    files: &[Written],
) -> String {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let mut tip: Option<String> = None;
    for file in files {
        let mut input = json!({
            "repositoryName": repository,
            "branchName": branch,
            "filePath": file.path,
            "fileContent": base64(file.content),
            "commitMessage": format!("add {}", file.path),
        });
        if let Some(parent) = &tip {
            input["parentCommitId"] = json!(parent);
        }
        let put = ok(router, "PutFile", &input).await;
        tip = Some(string_at(&put, "/commitId"));
    }
    tip.unwrap_or_else(|| panic!("case {repository}: the table writes at least one file"))
}

/// A `DeleteFile` request.
fn delete_request(
    repository: &str,
    branch: &str,
    sent_path: &str,
    parent: &str,
    keep_empty_folders: Option<bool>,
) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": sent_path,
        "parentCommitId": parent,
    });
    if let Some(keep) = keep_empty_folders {
        input["keepEmptyFolders"] = json!(keep);
    }
    input
}

/// The paths the tree holds after the deletion, as this file reads the rule: every
/// written path but the deleted one, plus — when `keepEmptyFolders` is true — one
/// placeholder in the deepest folder the deletion emptied, which is what keeps every
/// folder above it non-empty.
fn expected_paths(case: &Case) -> BTreeSet<String> {
    let mut paths: BTreeSet<String> = case.files.iter().map(|file| canonical(file.path)).collect();
    paths.remove(&case.deleted());
    if case.keeps() {
        for folder in ancestor_folders(&case.deleted()) {
            let prefix = format!("{folder}/");
            if paths.iter().any(|path| path.starts_with(&prefix)) {
                continue;
            }
            paths.insert(format!("{prefix}{PLACEHOLDER}"));
        }
    }
    paths
}

/// The placeholders [`expected_paths`] says this deletion wrote.
fn expected_placeholders(case: &Case) -> BTreeSet<String> {
    expected_paths(case)
        .into_iter()
        .filter(|path| path.ends_with(&format!("/{PLACEHOLDER}")))
        .collect()
}

/// A `GetFile` request for one path of a repository, at a commit specifier when one
/// is given.
fn get_file(repository: &str, path: &str, commit: Option<&str>) -> Value {
    let mut input = json!({ "repositoryName": repository, "filePath": path });
    if let Some(commit) = commit {
        input["commitSpecifier"] = json!(commit);
    }
    input
}

/// The entries one folder of a repository reports, as `relativePath -> blobId`.
async fn folder_entries(
    router: &MultiServiceRouter,
    repository: &str,
    folder: &str,
) -> BTreeMap<String, String> {
    let payload = ok(
        router,
        "GetFolder",
        &json!({ "repositoryName": repository, "folderPath": folder }),
    )
    .await;
    payload
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                string_at(entry, "/relativePath"),
                string_at(entry, "/blobId"),
            )
        })
        .collect()
}

#[tokio::test]
async fn delete_file_commits_the_tree_without_the_deleted_path() {
    let router = emulator();

    for case in cases() {
        let label = case.repository;
        let parent = write_files(&router, case.repository, case.branch, &case.files).await;
        let deleted = case.deleted();

        let request = delete_request(
            case.repository,
            case.branch,
            case.sent_path,
            &parent,
            case.keep_empty_folders,
        );
        let payload = ok(&router, "DeleteFile", &request).await;

        // The response names the path it deleted, the blob that left the tree and
        // the commit and tree the deletion wrote.
        assert_eq!(
            string_at(&payload, "/filePath"),
            deleted,
            "case {label}: filePath must be the path deleted, got {payload}"
        );
        assert_eq!(
            string_at(&payload, "/blobId"),
            git_blob_id(case.deleted_content()),
            "case {label}: blobId must be the git object name of the removed file's \
             content, got {payload}"
        );
        let commit_id = string_at(&payload, "/commitId");
        let tree_id = string_at(&payload, "/treeId");
        assert!(
            is_object_id(&commit_id),
            "case {label}: commitId must be a 40-character hex object id, got {commit_id:?}"
        );
        assert!(
            is_object_id(&tree_id),
            "case {label}: treeId must be a 40-character hex object id, got {tree_id:?}"
        );
        assert_ne!(
            commit_id, parent,
            "case {label}: the deletion is a NEW commit, got {payload}"
        );

        // "A commit is created on the branch that contains the revision": the branch
        // moves to that commit, and it is built on the parent the caller named.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.repository, "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            commit_id,
            "case {label}: the branch must point at the deletion commit, got {branch}"
        );
        let commit = ok(
            &router,
            "GetCommit",
            &json!({ "repositoryName": case.repository, "commitId": commit_id.clone() }),
        )
        .await;
        assert_eq!(
            commit.pointer("/commit/parents"),
            Some(&json!([parent.clone()])),
            "case {label}: the deletion commit must be built on the parent named, got {commit}"
        );
        assert_eq!(
            string_at(&commit, "/commit/treeId"),
            tree_id,
            "case {label}: the commit's tree must be the tree reported, got {commit}"
        );

        // The deleted path is gone from the branch...
        let code = error(
            &router,
            "GetFile",
            &get_file(case.repository, &deleted, None),
            400,
        )
        .await;
        assert_eq!(
            code, "FileDoesNotExistException",
            "case {label}: the deleted path must no longer be readable at the tip"
        );
        // ... and "the file still exists in the commits earlier to the commit that
        // contains the deletion".
        let earlier = ok(
            &router,
            "GetFile",
            &get_file(case.repository, &deleted, Some(&parent)),
        )
        .await;
        assert_eq!(
            from_base64(&string_at(&earlier, "/fileContent")),
            case.deleted_content(),
            "case {label}: the parent commit must still hold the deleted file"
        );

        // Everything else the tree held is untouched, byte for byte.
        let expected = expected_paths(&case);
        let placeholders = expected_placeholders(&case);
        for file in &case.files {
            let path = canonical(file.path);
            if path == deleted {
                continue;
            }
            let got = ok(
                &router,
                "GetFile",
                &get_file(case.repository, &path, None),
            )
            .await;
            assert_eq!(
                from_base64(&string_at(&got, "/fileContent")),
                file.content,
                "case {label}: {path} must be unchanged by the deletion"
            );
            assert_eq!(
                string_at(&got, "/blobId"),
                git_blob_id(file.content),
                "case {label}: {path} must keep its blob id"
            );
        }

        // A folder the deletion emptied is kept ONLY when the request asked for it,
        // and then by the placeholder the rule writes.
        for folder in ancestor_folders(&deleted) {
            let still_held = expected
                .iter()
                .any(|path| path.starts_with(&format!("{folder}/")));
            if still_held {
                let entries = folder_entries(&router, case.repository, folder).await;
                let placeholder = format!("{folder}/{PLACEHOLDER}");
                if placeholders.contains(&placeholder) {
                    assert_eq!(
                        entries.get(PLACEHOLDER).map(String::as_str),
                        Some(git_blob_id(&[]).as_str()),
                        "case {label}: the kept folder {folder} must hold the \
                         {PLACEHOLDER} placeholder, got {entries:?}"
                    );
                } else {
                    assert!(
                        !entries.contains_key(PLACEHOLDER),
                        "case {label}: the folder {folder} still holds a file, so no \
                         placeholder may be written, got {entries:?}"
                    );
                }
            } else {
                let code = error(
                    &router,
                    "GetFolder",
                    &json!({ "repositoryName": case.repository, "folderPath": folder }),
                    400,
                )
                .await;
                assert_eq!(
                    code, "FolderDoesNotExistException",
                    "case {label}: the emptied folder {folder} must be gone with the file"
                );
            }
        }

        // The stored tree is exactly the remaining snapshot: the root folder's tree
        // is the one the response reported, and the paths it reaches are the paths
        // the rule says are left.
        let root = ok(
            &router,
            "GetFolder",
            &json!({ "repositoryName": case.repository, "folderPath": "" }),
        )
        .await;
        assert_eq!(
            string_at(&root, "/treeId"),
            tree_id,
            "case {label}: the root folder's tree must be the tree the deletion wrote, \
             got {root}"
        );
        assert_eq!(
            reachable_paths(&router, case.repository, "").await,
            expected,
            "case {label}: the tree left behind must be exactly the remaining paths"
        );

        // A second repository holding the same content, on another branch, reports
        // the same ids for the same deletion: both are content-addressed, so this
        // checks the tree stored rather than the service against itself.
        let twin = format!("{label}-twin");
        let twin_parent = write_files(&router, &twin, "mirror", &case.files).await;
        let twin_payload = ok(
            &router,
            "DeleteFile",
            &delete_request(
                &twin,
                "mirror",
                case.sent_path,
                &twin_parent,
                case.keep_empty_folders,
            ),
        )
        .await;
        assert_eq!(
            string_at(&twin_payload, "/treeId"),
            tree_id,
            "case {label}: the same deletion over the same content must leave the same tree"
        );
        assert_eq!(
            string_at(&twin_payload, "/blobId"),
            string_at(&payload, "/blobId"),
            "case {label}: the removed blob is the same object in both repositories"
        );
    }
}

/// Every path the tree reaches under `folder`, walked through `GetFolder`.
async fn reachable_paths(
    router: &MultiServiceRouter,
    repository: &str,
    folder: &str,
) -> BTreeSet<String> {
    let payload = ok(
        router,
        "GetFolder",
        &json!({ "repositoryName": repository, "folderPath": folder }),
    )
    .await;
    let mut paths: BTreeSet<String> = payload
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string_at(entry, "/absolutePath"))
        .collect();
    let sub_folders: Vec<String> = payload
        .get("subFolders")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string_at(entry, "/absolutePath"))
        .collect();
    for sub in sub_folders {
        paths.extend(Box::pin(reachable_paths(router, repository, &sub)).await);
    }
    paths
}

#[tokio::test]
async fn successive_deletions_chain_through_the_branch_tip() {
    let router = emulator();
    let files = vec![
        Written {
            path: "one.txt",
            content: b"one\n",
        },
        Written {
            path: "two.txt",
            content: b"two\n",
        },
        Written {
            path: "three.txt",
            content: b"three\n",
        },
    ];
    let first_tip = write_files(&router, "sequence", "main", &files).await;

    // Each deletion has to name the CURRENT tip; the one before it is outdated.
    let mut tip = first_tip.clone();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        let case = format!("deletion {} of {}", index + 1, files.len());
        if index > 0 {
            let code = error(
                &router,
                "DeleteFile",
                &delete_request("sequence", "main", file.path, &first_tip, None),
                400,
            )
            .await;
            assert_eq!(
                code, "ParentCommitIdOutdatedException",
                "case {case}: a parent that is no longer the tip must be refused"
            );
        }
        let payload = ok(
            &router,
            "DeleteFile",
            &delete_request("sequence", "main", file.path, &tip, None),
        )
        .await;
        let commit_id = string_at(&payload, "/commitId");
        assert!(
            seen.insert(commit_id.clone()),
            "case {case}: every deletion is its own commit, got {commit_id} twice"
        );
        tip = commit_id;
    }

    // The last file left: the branch exists, points at the last deletion, and its
    // tree holds nothing.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "sequence", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "the branch must survive the deletion of its last file, got {branch}"
    );
    let root = ok(
        &router,
        "GetFolder",
        &json!({ "repositoryName": "sequence", "folderPath": "" }),
    )
    .await;
    assert_eq!(
        root.get("files"),
        Some(&json!([])),
        "every file was deleted, so the root tree reports an empty files list, got {root}"
    );
    assert_eq!(
        root.get("subFolders"),
        Some(&json!([])),
        "every file was deleted, so the root tree reports an empty subFolders list, got {root}"
    );

    // Deleting the same path again finds nothing to delete.
    let code = error(
        &router,
        "DeleteFile",
        &delete_request("sequence", "main", "one.txt", &tip, None),
        400,
    )
    .await;
    assert_eq!(
        code, "FileDoesNotExistException",
        "a path the tree no longer holds cannot be deleted again"
    );
}

#[tokio::test]
async fn the_deletion_commit_carries_the_caller_identity_and_message() {
    let router = emulator();
    let files = vec![
        Written {
            path: "named.txt",
            content: b"named\n",
        },
        Written {
            path: "default.txt",
            content: b"default\n",
        },
    ];
    let mut tip = write_files(&router, "identity", "main", &files).await;

    // With the optional members sent, the commit records exactly them.
    let mut request = delete_request("identity", "main", "named.txt", &tip, None);
    request["name"] = json!("Dev One");
    request["email"] = json!("dev.one@example.com");
    request["commitMessage"] = json!("drop the named file");
    let payload = ok(&router, "DeleteFile", &request).await;
    tip = string_at(&payload, "/commitId");
    let commit = ok(
        &router,
        "GetCommit",
        &json!({ "repositoryName": "identity", "commitId": tip.clone() }),
    )
    .await;
    for member in ["author", "committer"] {
        assert_eq!(
            string_at(&commit, &format!("/commit/{member}/name")),
            "Dev One",
            "case explicit identity: the {member} name is the one sent, got {commit}"
        );
        assert_eq!(
            string_at(&commit, &format!("/commit/{member}/email")),
            "dev.one@example.com",
            "case explicit identity: the {member} email is the one sent, got {commit}"
        );
    }
    assert_eq!(
        string_at(&commit, "/commit/message"),
        "drop the named file",
        "case explicit identity: the commit message is the one sent, got {commit}"
    );

    // With none of them sent: "If no name is specified, the user's ARN is used as
    // the author name and committer name" — recorded as that principal's NAME, the
    // git identity field a commit carries — "if no email address is specified, the
    // email address is left blank" and "if no message is specified, a default
    // message is used".
    let payload = ok(
        &router,
        "DeleteFile",
        &delete_request("identity", "main", "default.txt", &tip, None),
    )
    .await;
    let commit_id = string_at(&payload, "/commitId");
    let commit = ok(
        &router,
        "GetCommit",
        &json!({ "repositoryName": "identity", "commitId": commit_id }),
    )
    .await;
    let caller = crate::wire::caller_identity_name(crate::wire::DEFAULT_ACCESS_KEY_ID);
    for member in ["author", "committer"] {
        assert_eq!(
            string_at(&commit, &format!("/commit/{member}/name")),
            caller,
            "case default identity: the {member} name defaults to the caller's identity \
             name, got {commit}"
        );
        let email = commit
            .pointer(&format!("/commit/{member}/email"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            email.is_empty(),
            "case default identity: the {member} email is left blank, got {email:?}"
        );
    }
    let message = commit
        .pointer("/commit/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case default identity: a commit with no message sent gets a default one, got {commit}"
    );
}

#[tokio::test]
async fn a_deletion_that_cannot_be_applied_is_refused_and_persists_nothing() {
    let router = emulator();
    let files = vec![
        Written {
            path: "README.md",
            content: b"# demo\n",
        },
        Written {
            path: "src/main.rs",
            content: b"fn main() {}\n",
        },
    ];
    let tip = write_files(&router, "refusals", "main", &files).await;
    // A second commit, so there is a commit the repository HOLDS that is no longer
    // the tip of the branch.
    let older = write_files(&router, "two-commits", "main", &files).await;
    let first_commit = string_at(
        &ok(
            &router,
            "GetCommit",
            &json!({ "repositoryName": "two-commits", "commitId": older.clone() }),
        )
        .await,
        "/commit/parents/0",
    );

    // Each row is a request that cannot be applied, and the error the model names
    // for it. They differ along which part of the request is at fault: the
    // repository, the branch, the parent commit, the path.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            delete_request("absent-repo", "main", "README.md", &tip, None),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name outside the modeled pattern",
            delete_request("not a name!", "main", "README.md", &tip, None),
            "InvalidRepositoryNameException",
        ),
        (
            "no repository name at all",
            json!({
                "branchName": "main",
                "filePath": "README.md",
                "parentCommitId": tip.clone(),
            }),
            "RepositoryNameRequiredException",
        ),
        (
            "no branch name at all",
            json!({
                "repositoryName": "refusals",
                "filePath": "README.md",
                "parentCommitId": tip.clone(),
            }),
            "BranchNameRequiredException",
        ),
        (
            "a branch name that cannot be a git reference",
            delete_request("refusals", "has space", "README.md", &tip, None),
            "InvalidBranchNameException",
        ),
        (
            "a branch name holding a reference-name sequence git refuses",
            delete_request("refusals", "feature..old", "README.md", &tip, None),
            "InvalidBranchNameException",
        ),
        (
            "a branch the repository does not hold",
            delete_request("refusals", "no-such-branch", "README.md", &tip, None),
            "BranchDoesNotExistException",
        ),
        (
            "no file path at all",
            json!({
                "repositoryName": "refusals",
                "branchName": "main",
                "parentCommitId": tip.clone(),
            }),
            "PathRequiredException",
        ),
        (
            "a path that is only separators",
            delete_request("refusals", "main", "/", &tip, None),
            "PathRequiredException",
        ),
        (
            "a path with an empty segment",
            delete_request("refusals", "main", "src//main.rs", &tip, None),
            "InvalidPathException",
        ),
        (
            "a path that climbs out of its folder",
            delete_request("refusals", "main", "src/../README.md", &tip, None),
            "InvalidPathException",
        ),
        (
            "a path the tree does not hold",
            delete_request("refusals", "main", "src/absent.rs", &tip, None),
            "FileDoesNotExistException",
        ),
        (
            "a path that names a folder rather than a file",
            delete_request("refusals", "main", "src", &tip, None),
            "FileDoesNotExistException",
        ),
        (
            "no parent commit id at all",
            json!({
                "repositoryName": "refusals",
                "branchName": "main",
                "filePath": "README.md",
            }),
            "ParentCommitIdRequiredException",
        ),
        (
            "a parent commit id that is not an object name",
            delete_request("refusals", "main", "README.md", "not-a-commit", None),
            "InvalidParentCommitIdException",
        ),
        (
            "an empty parent commit id",
            delete_request("refusals", "main", "README.md", "", None),
            "InvalidParentCommitIdException",
        ),
        (
            "a parent commit id the repository does not hold",
            delete_request("refusals", "main", "README.md", &"0".repeat(40), None),
            "ParentCommitDoesNotExistException",
        ),
        (
            "a parent commit that is not the tip of the branch",
            delete_request("two-commits", "main", "README.md", &first_commit, None),
            "ParentCommitIdOutdatedException",
        ),
    ];

    for (case, request, expected) in cases {
        let code = error(&router, "DeleteFile", &request, 400).await;
        assert_eq!(
            code, expected,
            "case {case:?} ({request}): expected {expected}, got {code}"
        );
    }

    // Nothing above was applied: the branch tip stands and every file is still
    // readable with the content it was written with.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "refusals", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "a refused deletion must not move the branch tip, got {branch}"
    );
    for file in &files {
        let got = ok(
            &router,
            "GetFile",
            &get_file("refusals", &canonical(file.path), None),
        )
        .await;
        assert_eq!(
            from_base64(&string_at(&got, "/fileContent")),
            file.content,
            "a refused deletion must leave {} in place",
            file.path
        );
    }
}

#[tokio::test]
async fn a_branch_name_that_names_a_tag_is_refused_as_a_tag_name() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State;

    let (router, engine) = emulator_with_engine();
    let files = vec![Written {
        path: "README.md",
        content: b"# demo\n",
    }];
    let tip = write_files(&router, "tagged", "main", &files).await;

    // A tag reaches a repository only through the git transport (CodeCommit has no
    // control-plane API that creates one), so the reference is seeded straight into
    // the store the handlers read, the way the branch suites seed a reference set.
    {
        let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
        let mut store = state.lock();
        let repository = store
            .repositories
            .get_mut("tagged")
            .expect("the repository was created above");
        repository.git_tags.insert("v1.0".to_string(), tip.clone());
    }

    let code = error(
        &router,
        "DeleteFile",
        &delete_request("tagged", "v1.0", "README.md", &tip, None),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchNameIsTagNameException",
        "a name the repository holds as a TAG is not a branch name"
    );

    // And a name it holds in neither namespace is simply no branch of it.
    let code = error(
        &router,
        "DeleteFile",
        &delete_request("tagged", "v2.0", "README.md", &tip, None),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a name held in neither namespace names no branch"
    );

    // Neither refusal touched the file.
    let got = ok(&router, "GetFile", &get_file("tagged", "README.md", None)).await;
    assert_eq!(
        from_base64(&string_at(&got, "/fileContent")),
        files[0].content,
        "a refused deletion must leave the file in place"
    );
}
