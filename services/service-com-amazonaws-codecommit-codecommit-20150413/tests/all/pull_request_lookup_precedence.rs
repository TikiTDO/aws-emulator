// SPDX-License-Identifier: CC0-1.0

//! Which of two lookups decides the refusal, over the wire: an operation that
//! takes BOTH a `pullRequestId` and a `repositoryName` resolves the REPOSITORY
//! first, so a request naming a repository the account does not hold is told that —
//! with `RepositoryDoesNotExistException` and the `<repositoryName> for <accountId>`
//! text CodeCommit words that code with — even when the pull request id it carried
//! names no pull request either.
//!
//! The rule under test is about precedence rather than about one request: a pull
//! request lives IN a repository, so the repository the request is scoped to is the
//! resource resolved before anything is looked up inside it. The table below
//! therefore varies what the rule generalises over — the operation that takes both
//! members, the shape of the repository name that names no repository, and the shape
//! of the id that names no pull request — and asserts the same code and message on
//! every row.
//!
//! The complementary halves are pinned next door: an id that names no pull request
//! in a repository that DOES exist is `PullRequestDoesNotExistException`
//! (`crate::pull_request_does_not_exist_message`, which mints a pull request so the
//! repository is real), an id belonging to another repository is
//! `RepositoryNotAssociatedWithPullRequestException` (`crate::pull_request_comments`),
//! and `crate::client_error_messages` pins this code's template.

use serde_json::{json, Value};

use crate::harness::error_with_message;
use crate::repository_lifecycle::emulator;

/// The HTTP status `RepositoryDoesNotExistException` models (`errorKind: client`,
/// `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation taking
/// a commit id gets past its own `InvalidCommitIdException` check and reaches the
/// lookups under test.
const AFTER_COMMIT_ID: &str = "0123456789abcdef0123456789abcdef01234567";
const BEFORE_COMMIT_ID: &str = "76543210fedcba9876543210fedcba9876543210";

/// Repository names that are valid `RepositoryName`s (1..100 of `[\w\.-]+`:
/// `smithy_query(query="shape",shape="RepositoryName")`) and name no repository on
/// an emulator that holds none. Rows differ along length and spelling.
fn absent_repositories() -> Vec<String> {
    vec![
        "q7".to_string(),
        "no-such-repository".to_string(),
        "no.such_repository-in-this-much-longer-name".to_string(),
        "r".repeat(100),
    ]
}

/// Pull request ids that are ids (`PullRequestId` is a decimal string: see
/// `crate::state::normalize_pull_request_id`) and name no pull request on an
/// emulator that has minted none.
fn absent_pull_request_ids() -> Vec<&'static str> {
    vec!["1", "999999", "18446744073709551615"]
}

/// The message CodeCommit words `RepositoryDoesNotExistException` with: the
/// requested name, the fixed `" for "`, then the account the lookup ran in.
fn expected_message(repository: &str) -> String {
    format!("{repository} for {}", engine::emulator_account_id())
}

/// Every operation that takes both a `pullRequestId` and a `repositoryName`, with
/// the rest of its required members filled in so each reaches the two lookups.
fn requests_naming(repository: &str, pull_request_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "PostCommentForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
                "content": "a comment",
            }),
        ),
        (
            "GetCommentsForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
                // Naming the repository makes both sides of the comparison
                // required on this operation, so both are filled in.
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
            }),
        ),
        (
            "MergePullRequestByFastForward",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
        (
            "MergePullRequestBySquash",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
        (
            "MergePullRequestByThreeWay",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": repository,
            }),
        ),
    ]
}

#[tokio::test]
async fn the_repository_lookup_comes_before_the_pull_request_lookup() {
    let router = emulator();

    for repository in absent_repositories() {
        for pull_request_id in absent_pull_request_ids() {
            for (operation, body) in requests_naming(&repository, pull_request_id) {
                let (code, message) =
                    error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
                assert_eq!(
                    code, "RepositoryDoesNotExistException",
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): the \
                     repository is resolved first, so expected code \
                     RepositoryDoesNotExistException, got {code:?}"
                );
                let expected = expected_message(&repository);
                assert_eq!(
                    message, expected,
                    "case ({operation}, repository {repository:?}, id {pull_request_id}): \
                     expected message {expected:?}, got {message:?}"
                );
            }
        }
    }
}
