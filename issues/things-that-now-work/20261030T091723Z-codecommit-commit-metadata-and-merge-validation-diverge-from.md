Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-file-folder-blob-content-operations
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-file-folder-blob-content-operations codecommit-merge-by-three-way-nonexistent-repository
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|ListFileCommitHistory|2xx->2xx|$.revisionDag[*]
Issue-Key: codecommit|MergePullRequestByThreeWay|4xx->4xx|$.__type
Issue-Key: codecommit|MergePullRequestByThreeWay|4xx->4xx|$.message
Issue-Commit: 58bd0c9b241d51644119b64ebf0df021c2b44f91

# CodeCommit commit metadata and merge validation diverge from AWS responses (2 causes)

> _Note: this issue was identified against the code base at commit `58bd0c9b241d51644119b64ebf0df021c2b44f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFileCommitHistory

## Description
TWO net-new wire causes in the codecommit emulator, neither owned by an open issue: the commit-identity fields the emulator stamps onto commits it creates and then serves back, and the order in which a pull-request merge operation validates its inputs. A fixer must close BOTH before submitting.

=== Cause 1 of 2: ListFileCommitHistory serves the full caller ARN as the commit author/committer identity ===
AWS: Smithy ListFileCommitHistory returns revisionDag: list<RevisionDag entry> whose `commit` member is the Commit shape, whose `author`/`committer` are UserInfo{name, email, date} (name -> Name, email -> Email, date -> Date, "timestamp format with GMT offset"). PutFile/CreateCommit take optional `name` and `email` members that populate those fields; the model does not state what is used when they are omitted, so this is recorded behavior only: for a commit created by a PutFile that omitted name/email, AWS reports author.name and author.email (and the identical committer.name/committer.email) as the caller's identity NAME — the final segment of the caller principal ARN (for arn:aws:iam::<account>:user/<user-name> that is <user-name>) — not the whole ARN. The date is a per-run timestamp and is not a requirement.
Emulator: for the same commit the emulator returns author.name, author.email, committer.name and committer.email all set to the full principal ARN (recorded step: 'arn:aws:iam::000000000000:user/emulator' in all four fields, where the recording has the identity name in all four). Every other field of the revisionDag entry (blobId, path, revisionChildren, commit.commitId, commit.treeId, commit.parents, commit.message, additionalData) matches; only the four identity strings diverge. Hypothesis: the commit-creation path stores the caller ARN verbatim as the git author/committer identity instead of deriving the identity name from it, and ListFileCommitHistory faithfully serves what was stored.
Why: a client that renders commit history, maps commits to users, or matches an author/committer against a user name (the value git itself shows) sees an ARN where AWS gives a user name, so author attribution, grouping by author and any equality test on the author of a commit the emulator created are all wrong. Because the value is written at commit time, every operation that returns a Commit (GetCommit, GetDifferences-adjacent history views, ListFileCommitHistory) inherits it.
Fix: the commit-identity derivation in the commit-creating handlers' state layer for the codecommit crate (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/put_file.rs and the shared commit-builder it calls), so the stored author/committer name and email are the caller's identity name rather than the full ARN; the serializer for ListFileCommitHistory needs no change.
Traces: codecommit-file-folder-blob-content-operations#task#9.

=== Cause 2 of 2: MergePullRequestByThreeWay validates the pull request before the repository, so a nonexistent repository raises the wrong error class ===
AWS: Smithy MergePullRequestByThreeWay requires BOTH pullRequestId and repositoryName (repositoryName @length 1..100, @pattern ^[\w\.-]+$) and models RepositoryDoesNotExistException (errorKind client, httpError 400 defaulted, doc "The specified repository does not exist.") alongside PullRequestDoesNotExistException. Recorded behavior: called with a repositoryName that does not exist, AWS answers HTTP 400 with __type 'RepositoryDoesNotExistException' and message of the form '<repositoryName> for <accountId>' — the fixed part is the literal ' for ' separator, and the variable parts are the repositoryName taken from the request and the 12-digit account id of the caller (recorded run: 'sim2real-probe-codecommit-cc-merge-threeway-repo-5d503b9e for 000000000000'; that repository name and account id are values of that run, not part of the template). The repository check precedes the pull-request lookup.
Emulator: HTTP 400 but __type 'PullRequestDoesNotExistException' with message 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.' — it resolves the pull request id first and never reports the missing repository. Hypothesis: the handler's validation order puts the pull-request lookup ahead of the repository existence check.
Why: a client branches on the modeled error type — a missing repository is an operator/config fault to surface (create the repo, fix the name), while PullRequestDoesNotExistException sends it hunting for a pull request id that may be perfectly valid. Retry/remediation logic keyed on RepositoryDoesNotExistException never fires, and the message loses the repository name and account id AWS reports. Note the related open issue 20261029T222310Z-codecommit-pullrequestdoesnotexistexception-carries-the-smithy-doc-string-instead covers the WORDING of PullRequestDoesNotExistException on other operations; it does not cover this one, which is a wrong error CLASS caused by validation order.
Fix: the input-validation order in the MergePullRequestByThreeWay handler (services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/merge_pull_request_by_three_way.rs): check repositoryName existence and raise RepositoryDoesNotExistException with AWS's '<repositoryName> for <accountId>' message before any pull-request lookup.
Traces: codecommit-merge-by-three-way-nonexistent-repository#task#1.

## Evidence
Cause 1 — fixture codecommit-file-folder-blob-content-operations step task#9 (ListFileCommitHistory), status 200 expected and 200 returned, body diverges at $.revisionDag[0]:
  Expected (recorded AWS): commit.author = {name: 'emulator', email: 'emulator', date: <per-run timestamp>}, commit.committer identical.
  Actual (emulator): commit.author = {name: 'arn:aws:iam::000000000000:user/emulator', email: 'arn:aws:iam::000000000000:user/emulator', date: '1793351731 +0000'}, commit.committer identical.
  Requirement: the identity NAME (final ARN segment), in name and email, for both author and committer. The 'emulator'/'000000000000'/timestamp values are that run's; the date field itself is a per-run value and is not asserted here.

Cause 2 — fixture codecommit-merge-by-three-way-nonexistent-repository step task#1 (MergePullRequestByThreeWay against a repository that does not exist), status 400 expected and 400 returned:
  Expected: $.__type 'RepositoryDoesNotExistException', $.message '<repositoryName> for <accountId>' (recorded: 'sim2real-probe-codecommit-cc-merge-threeway-repo-5d503b9e for 000000000000').
  Actual: $.__type 'PullRequestDoesNotExistException', $.message 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.'
  Fixed vs variable in the expected message: ' for ' is fixed text; the repository name comes from the request and the account id from the caller's identity.

## Steps to Reproduce
1. Replay fixture codecommit-file-folder-blob-content-operations. Through task#8 the fixture creates a repository and commits docs/sub/notes.txt via PutFile without `name`/`email`; at step task#9 it calls ListFileCommitHistory for that path and compares $.revisionDag[0] — commit.author.name/email and commit.committer.name/email come back as the full caller ARN instead of the caller's identity name.
2. Replay fixture codecommit-merge-by-three-way-nonexistent-repository step task#1: call MergePullRequestByThreeWay with a repositoryName that was never created. Compare $.__type and $.message with the recording: the emulator returns PullRequestDoesNotExistException instead of RepositoryDoesNotExistException '<repositoryName> for <accountId>'.
3. Both steps are in trace set simparity-wire-aws-v1 and neither is blocked by a stub nor owned by an open issue in the replay report.
