// SPDX-License-Identifier: CC0-1.0

//! Every `ObjectId` CodeCommit reports is the git object NAME of the object it
//! describes: SHA-1 over the git object encoding `"<kind> <len>\0<payload>"`.
//!
//! The rule under test, stated once: a `blobId` is `sha1("blob <len>\0" + bytes)`
//! of the stored bytes, a `treeId` is the git tree object id of the resulting tree
//! (one real subtree object per path component, entries ordered by name with a
//! subtree's name compared as if it ended in `/`, mode `100644` for NORMAL,
//! `100755` for EXECUTABLE and `120000` for SYMLINK), and a `commitId` is the git
//! commit object id over that tree, its parents, its identities and its message.
//! A client can therefore check a returned id with `git hash-object` /
//! `git write-tree`.
//!
//! Each test checks that rule two ways: against an encoder written here,
//! independently of the service, and — where git itself publishes the value —
//! against the literal object names from the Pro Git book's "Git Internals"
//! chapter, which is an anchor the emulator cannot influence.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness — building the emulator, signing a call, reading an answer and
// the small value helpers — is the shared one in `crate::repository_lifecycle`, so
// no two suites can disagree about what calling the service means. Only the git
// object encoder below is written again here, deliberately: it is the oracle this
// suite checks the service against.
use crate::repository_lifecycle::{
    base64, emulator, emulator_with_engine, error, git_blob_id, ok, string_at,
};

/// Write one file and return the `PutFile` response.
async fn put_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    path: &str,
    content: &[u8],
    mode: Option<&str>,
    parent: Option<&str>,
) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": path,
        "fileContent": base64(content),
        "commitMessage": format!("write {path}"),
        "name": "Test Author",
        "email": "author@example.com",
    });
    if let Some(mode) = mode {
        input["fileMode"] = json!(mode);
    }
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    ok(router, "PutFile", &input).await
}

// ---------------------------------------------------------------------------
// The git object encoding, written here independently of the service.
// ---------------------------------------------------------------------------

/// The name of a git object: SHA-1 over `"<kind> <len>\0"` and the payload.
///
/// `pub(crate)` so the sibling suite that checks WHICH DATE a commit carries
/// ([`crate::commit_date_identity`]) names a commit with this one oracle rather
/// than a second copy of it.
pub(crate) fn object_name(kind: &str, payload: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{kind} {}\0", payload.len()).as_bytes());
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

/// One tree, flattened: path (relative to this tree) -> (git mode, object name).
type Entries = BTreeMap<String, (&'static str, String)>;

/// How a tree's entries are ordered before they are hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryOrder {
    /// What git does: a subtree's name sorts as if it ended in `/`.
    Git,
    /// A naive alternative, kept only so a test can show the service does NOT
    /// produce it: order by the bare entry names.
    BareName,
}

fn git_tree_payload(entries: &Entries, order: EntryOrder) -> Vec<u8> {
    let mut rows: Vec<(Vec<u8>, String, String)> = Vec::new();
    let mut subtrees: BTreeMap<String, Entries> = BTreeMap::new();
    for (path, (mode, id)) in entries {
        match path.split_once('/') {
            None => rows.push((
                path.as_bytes().to_vec(),
                format!("{mode} {path}"),
                id.clone(),
            )),
            Some((head, rest)) => {
                subtrees
                    .entry(head.to_string())
                    .or_default()
                    .insert(rest.to_string(), (mode, id.clone()));
            }
        }
    }
    for (name, subtree) in &subtrees {
        let mut key = name.as_bytes().to_vec();
        if order == EntryOrder::Git {
            key.push(b'/');
        }
        rows.push((
            key,
            format!("40000 {name}"),
            object_name("tree", &git_tree_payload(subtree, order)),
        ));
    }
    rows.sort_by(|left, right| left.0.cmp(&right.0));

    let mut payload: Vec<u8> = Vec::new();
    for (_, header, id) in rows {
        payload.extend_from_slice(header.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&hex::decode(&id).expect("an object name is hex"));
    }
    payload
}

fn git_tree_id(entries: &Entries) -> String {
    object_name("tree", &git_tree_payload(entries, EntryOrder::Git))
}

/// The git mode of a modeled `fileMode`. The enum's three values are from
/// `smithy_query(query="shape",shape="FileModeTypeEnum")`.
fn git_mode(file_mode: &str) -> &'static str {
    match file_mode {
        "EXECUTABLE" => "100755",
        "SYMLINK" => "120000",
        "NORMAL" => "100644",
        other => panic!("fileMode {other:?} is not a FileModeTypeEnum value"),
    }
}

// Object names git itself publishes, used as external anchors. Source: the Pro
// Git book, "Git Internals — Git Objects" (`git hash-object` / `git write-tree`
// transcripts). The empty-blob and empty-tree names are git's well-known
// constants.
const EMPTY_BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const BLOB_TEST_CONTENT: (&str, &str) = ("test content\n", "d670460b4b4aece5915caf5c68d12f560a9fe3e4");
const BLOB_VERSION_1: (&str, &str) = ("version 1\n", "83baae61804e65cc73a7201a7252750c76066a30");
const BLOB_VERSION_2: (&str, &str) = ("version 2\n", "1f7a7a472abf3dd9643fd615f6da379c4acb3e3a");
const BLOB_NEW_FILE: (&str, &str) = ("new file\n", "fa49b077972391ad58037050f2a75f74e3671e92");
const BLOB_WHAT_IS_UP: (&str, &str) = (
    "what is up, doc?",
    "bd9dbf5aae1a3862dd1526723246b20206e5fc37",
);
/// `git write-tree` of a tree holding only `test.txt` at [`BLOB_VERSION_1`].
const TREE_TEST_V1: &str = "d8329fc1cc938780ffdd9f94e0d364e0ea74f579";
/// ... of `new.txt` at [`BLOB_NEW_FILE`] plus `test.txt` at [`BLOB_VERSION_2`].
const TREE_NEW_AND_TEST_V2: &str = "0155eb4229851634a0f03eb265b69f5a2d56f341";
/// ... of the tree above plus the subtree `bak/` holding [`TREE_TEST_V1`].
const TREE_WITH_BAK_SUBTREE: &str = "3c4e9cd789d88d8d89c1073707c3585e41b0e614";

/// The encoder above, checked against the object names git publishes, so every
/// other test in this file rests on a verified reference implementation.
#[test]
fn the_reference_encoder_agrees_with_the_object_names_git_publishes() {
    assert_eq!(
        git_blob_id(b""),
        EMPTY_BLOB,
        "case \"empty blob\": expected git's empty-blob name, got {:?}",
        git_blob_id(b"")
    );
    assert_eq!(
        git_tree_id(&Entries::new()),
        EMPTY_TREE,
        "case \"empty tree\": expected git's empty-tree name, got {:?}",
        git_tree_id(&Entries::new())
    );
    for (content, expected) in [
        BLOB_TEST_CONTENT,
        BLOB_VERSION_1,
        BLOB_VERSION_2,
        BLOB_NEW_FILE,
        BLOB_WHAT_IS_UP,
    ] {
        let actual = git_blob_id(content.as_bytes());
        assert_eq!(
            actual, expected,
            "case {content:?}: expected blob name {expected}, got {actual:?}"
        );
    }

    let mut tree: Entries = Entries::new();
    tree.insert(
        "test.txt".to_string(),
        ("100644", BLOB_VERSION_1.1.to_string()),
    );
    let actual = git_tree_id(&tree);
    assert_eq!(
        actual, TREE_TEST_V1,
        "case \"tree{{test.txt}}\": expected {TREE_TEST_V1}, got {actual:?}"
    );

    tree.insert(
        "test.txt".to_string(),
        ("100644", BLOB_VERSION_2.1.to_string()),
    );
    tree.insert(
        "new.txt".to_string(),
        ("100644", BLOB_NEW_FILE.1.to_string()),
    );
    let actual = git_tree_id(&tree);
    assert_eq!(
        actual, TREE_NEW_AND_TEST_V2,
        "case \"tree{{new.txt,test.txt}}\": expected {TREE_NEW_AND_TEST_V2}, got {actual:?}"
    );

    tree.insert(
        "bak/test.txt".to_string(),
        ("100644", BLOB_VERSION_1.1.to_string()),
    );
    let actual = git_tree_id(&tree);
    assert_eq!(
        actual, TREE_WITH_BAK_SUBTREE,
        "case \"tree{{bak/,new.txt,test.txt}}\": expected {TREE_WITH_BAK_SUBTREE}, got {actual:?}"
    );
}

/// One row of the blob table: bytes written, and the name git publishes for them
/// where there is one.
struct BlobCase {
    label: &'static str,
    path: &'static str,
    content: Vec<u8>,
    published: Option<&'static str>,
}

/// The table spans what the rule generalises over: content size (1 byte, a short
/// line, 256 bytes, 100 KiB), byte values (text, every byte including NUL,
/// multi-byte UTF-8) and the presence of a published anchor.
fn blob_cases() -> Vec<BlobCase> {
    vec![
        BlobCase {
            label: "a published one-line text blob",
            path: "test.txt",
            content: BLOB_TEST_CONTENT.0.as_bytes().to_vec(),
            published: Some(BLOB_TEST_CONTENT.1),
        },
        BlobCase {
            label: "another published text blob",
            path: "doc.txt",
            content: BLOB_WHAT_IS_UP.0.as_bytes().to_vec(),
            published: Some(BLOB_WHAT_IS_UP.1),
        },
        BlobCase {
            label: "a single byte",
            path: "one.bin",
            content: vec![b'x'],
            published: None,
        },
        BlobCase {
            label: "every byte value, NUL included",
            path: "deep/dir/all-bytes.bin",
            content: (0..=255u8).collect(),
            published: None,
        },
        BlobCase {
            label: "multi-byte utf-8",
            path: "docs/guía.md",
            content: "⚙ configuración\n".as_bytes().to_vec(),
            published: None,
        },
        BlobCase {
            label: "100 KiB of repeating bytes",
            path: "data/large.bin",
            content: (0..=255u8).cycle().take(100 * 1024).collect(),
            published: None,
        },
    ]
}

/// `blobId` is the git object name of the bytes written, and `treeId` is the git
/// tree object name of the tree each write produces — checked after every write,
/// so the tree is verified as it grows from one entry to several across two
/// subtree levels.
#[tokio::test]
async fn blob_and_tree_ids_are_the_git_object_names_of_what_was_written() {
    let router = emulator();
    let repository = "object-ids";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let mut expected_tree: Entries = Entries::new();
    let mut parent: Option<String> = None;
    let mut seen_blobs: BTreeSet<String> = BTreeSet::new();

    for case in blob_cases() {
        let label = case.label;
        let reference = git_blob_id(&case.content);
        if let Some(published) = case.published {
            assert_eq!(
                reference, published,
                "case {label:?}: the reference encoder must agree with git's published \
                 name {published}, got {reference:?}"
            );
        }

        let put = put_file(
            &router,
            repository,
            "main",
            case.path,
            &case.content,
            None,
            parent.as_deref(),
        )
        .await;
        let blob_id = string_at(&put, "/blobId");
        assert_eq!(
            blob_id, reference,
            "case {label:?}: blobId must be sha1(\"blob {}\\0\" + bytes) = {reference}, got \
             {blob_id:?}",
            case.content.len()
        );
        seen_blobs.insert(blob_id.clone());

        expected_tree.insert(case.path.to_string(), ("100644", blob_id.clone()));
        let expected_root = git_tree_id(&expected_tree);
        let tree_id = string_at(&put, "/treeId");
        assert_eq!(
            tree_id, expected_root,
            "case {label:?}: treeId must be the git tree name of the resulting tree \
             {expected_root}, got {tree_id:?}"
        );

        // The reader reports the same name for the same bytes.
        let got = ok(
            &router,
            "GetFile",
            &json!({ "repositoryName": repository, "filePath": case.path }),
        )
        .await;
        assert_eq!(
            string_at(&got, "/blobId"),
            reference,
            "case {label:?}: GetFile must report the same object name as the write"
        );

        parent = Some(string_at(&put, "/commitId"));
    }

    assert_eq!(
        seen_blobs.len(),
        blob_cases().len(),
        "case \"distinct contents\": distinct bytes must get distinct object names, got \
         {seen_blobs:?}"
    );
}

/// The `git write-tree` transcript git publishes, driven through `PutFile`: the
/// root tree the service reports after each write is the tree name git records
/// for exactly that content.
#[tokio::test]
async fn the_published_write_tree_transcript_is_reproduced_over_the_wire() {
    let router = emulator();
    let repository = "progit";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    // (path, content, the root tree name git publishes after this write)
    let steps: Vec<(&str, &str, Option<&str>)> = vec![
        ("test.txt", BLOB_VERSION_1.0, Some(TREE_TEST_V1)),
        ("test.txt", BLOB_VERSION_2.0, None),
        ("new.txt", BLOB_NEW_FILE.0, Some(TREE_NEW_AND_TEST_V2)),
        (
            "bak/test.txt",
            BLOB_VERSION_1.0,
            Some(TREE_WITH_BAK_SUBTREE),
        ),
    ];

    let mut parent: Option<String> = None;
    let mut trees: Vec<String> = Vec::new();
    for (path, content, published_tree) in steps {
        let put = put_file(
            &router,
            repository,
            "main",
            path,
            content.as_bytes(),
            None,
            parent.as_deref(),
        )
        .await;
        let tree_id = string_at(&put, "/treeId");
        if let Some(published) = published_tree {
            assert_eq!(
                tree_id, published,
                "case ({path:?}, {content:?}): treeId must be git's published root tree \
                 {published}, got {tree_id:?}"
            );
        }
        trees.push(tree_id);
        parent = Some(string_at(&put, "/commitId"));
    }

    assert_eq!(
        trees.iter().collect::<BTreeSet<_>>().len(),
        trees.len(),
        "case \"every write changed the tree\": each of these writes changes the content, so \
         each root tree differs, got {trees:?}"
    );
}

/// A `fileMode` change alone changes the `treeId`, because the mode is part of
/// the tree entry git hashes — and the content it points at is unchanged, so the
/// `blobId` does not move.
#[tokio::test]
async fn the_file_mode_alone_changes_the_tree_id() {
    let router = emulator();
    let repository = "modes";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let content = b"#!/bin/sh\necho hello from the emulator\n";
    let expected_blob = git_blob_id(content);
    let path = "bin/run.sh";

    let mut parent: Option<String> = None;
    let mut trees: BTreeMap<&str, String> = BTreeMap::new();
    // Every value `FileModeTypeEnum` declares, each with the git mode bits git
    // writes for it.
    for mode in ["NORMAL", "EXECUTABLE", "SYMLINK"] {
        let put = put_file(
            &router,
            repository,
            "main",
            path,
            content,
            Some(mode),
            parent.as_deref(),
        )
        .await;
        let blob_id = string_at(&put, "/blobId");
        assert_eq!(
            blob_id, expected_blob,
            "case {mode:?}: the mode does not change the content, so blobId stays \
             {expected_blob}, got {blob_id:?}"
        );

        let mut expected_tree = Entries::new();
        expected_tree.insert(path.to_string(), (git_mode(mode), blob_id));
        let expected_root = git_tree_id(&expected_tree);
        let tree_id = string_at(&put, "/treeId");
        assert_eq!(
            tree_id, expected_root,
            "case {mode:?}: treeId must be the git tree name with mode {} = {expected_root}, \
             got {tree_id:?}",
            git_mode(mode)
        );
        trees.insert(mode, tree_id);
        parent = Some(string_at(&put, "/commitId"));

        // Re-writing identical content in the identical mode changes nothing, so
        // there is no new tree to report.
        let code = error(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": "main",
                "filePath": path,
                "fileContent": base64(content),
                "fileMode": mode,
                "parentCommitId": parent.as_deref().expect("the write reported a commit"),
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "SameFileContentException",
            "case {mode:?}: an identical content-and-mode write must be refused"
        );
    }

    let distinct: BTreeSet<&String> = trees.values().collect();
    assert_eq!(
        distinct.len(),
        trees.len(),
        "case \"mode changes the tree\": each file mode must give its own treeId, got {trees:?}"
    );
}

/// git orders tree entries by name, comparing a subtree's name as if it ended in
/// `/` — which is what puts `a.txt` before the subtree `a/`. A tree holding both
/// therefore has the id git computes, and NOT the one a bare-name ordering gives.
#[tokio::test]
async fn tree_entries_are_ordered_the_way_git_orders_them() {
    let router = emulator();
    let repository = "ordering";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    // Names chosen so the two orderings disagree: '.' and '-' sort before '/',
    // and 'a' after it.
    let paths = [
        "a.txt",
        "a/b.txt",
        "a-b.txt",
        "aa.txt",
        "a/c/d.txt",
        "zz.txt",
    ];
    let mut expected_tree = Entries::new();
    let mut parent: Option<String> = None;
    let mut last_tree = String::new();
    for path in paths {
        let content = format!("content of {path}\n").into_bytes();
        let put = put_file(
            &router,
            repository,
            "main",
            path,
            &content,
            None,
            parent.as_deref(),
        )
        .await;
        expected_tree.insert(path.to_string(), ("100644", git_blob_id(&content)));
        let expected_root = git_tree_id(&expected_tree);
        last_tree = string_at(&put, "/treeId");
        assert_eq!(
            last_tree, expected_root,
            "case {path:?}: treeId must be git's tree name {expected_root}, got {last_tree:?}"
        );
        parent = Some(string_at(&put, "/commitId"));
    }

    let bare_name_order = object_name(
        "tree",
        &git_tree_payload(&expected_tree, EntryOrder::BareName),
    );
    assert_ne!(
        last_tree, bare_name_order,
        "case \"file before subtree\": a tree holding both a.txt and a/ must use git's \
         ordering, not the bare-name ordering {bare_name_order}"
    );
}

/// `commitId` is the git commit object name over the tree, the parents, the two
/// identities and the message: changing any of them changes the id, and the id is
/// reproducible from what the service stored.
#[tokio::test]
async fn commit_ids_are_the_git_object_name_of_the_commit() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State;

    let (router, engine) = emulator_with_engine();
    let repository = "commits";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let mut parent: Option<String> = None;
    let mut commits: Vec<String> = Vec::new();
    for (path, content) in [
        ("test.txt", BLOB_VERSION_1.0),
        ("test.txt", BLOB_VERSION_2.0),
        ("new.txt", BLOB_NEW_FILE.0),
    ] {
        let put = put_file(
            &router,
            repository,
            "main",
            path,
            content.as_bytes(),
            None,
            parent.as_deref(),
        )
        .await;
        let commit_id = string_at(&put, "/commitId");

        // Recompute the commit object from what the service persisted. Nothing
        // here reads the service's own id computation.
        let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
        let store = state.lock();
        let stored = store
            .repositories
            .get(repository)
            .and_then(|repository| repository.commits.get(&commit_id))
            .unwrap_or_else(|| panic!("case {path:?}: the commit {commit_id} was not stored"))
            .clone();
        drop(store);

        let identity = |user: &service_com_amazonaws_codecommit_codecommit_20150413::ops::types::UserInfo| {
            format!(
                "{} <{}> {}",
                user.name.as_deref().unwrap_or(""),
                user.email.as_deref().unwrap_or(""),
                user.date.as_deref().unwrap_or("0 +0000")
            )
        };
        let mut payload = format!("tree {}\n", stored.tree_id);
        for parent in &stored.parents {
            payload.push_str(&format!("parent {parent}\n"));
        }
        payload.push_str(&format!("author {}\n", identity(&stored.author)));
        payload.push_str(&format!("committer {}\n", identity(&stored.committer)));
        payload.push('\n');
        payload.push_str(stored.message.as_deref().unwrap_or(""));
        let expected = object_name("commit", payload.as_bytes());
        assert_eq!(
            commit_id, expected,
            "case {path:?}: commitId must be the git commit object name {expected}, got \
             {commit_id:?}"
        );

        assert_eq!(
            stored.parents,
            parent.clone().into_iter().collect::<Vec<String>>(),
            "case {path:?}: the commit's parent must be the branch tip it was written onto"
        );
        commits.push(commit_id.clone());
        parent = Some(commit_id);
    }

    assert_eq!(
        commits.iter().collect::<BTreeSet<_>>().len(),
        commits.len(),
        "case \"distinct commits\": each commit differs in tree or parent, so each id differs, \
         got {commits:?}"
    );
}
