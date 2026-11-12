Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-file-folder-blob-content-operations
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-file-folder-blob-content-operations
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|ListFileCommitHistory|2xx->2xx|$.revisionDag[*]
Issue-Commit: 97bcd61576aa0f22fd1da9220056da036e0a415a

# CodeCommit ListFileCommitHistory reports a revisionDag commit date that disagrees with the commit's own recorded author/committer date

> _Note: this issue was identified against the code base at commit `97bcd61576aa0f22fd1da9220056da036e0a415a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFileCommitHistory

## Description
One net-new wire cause in the codecommit crate, from a replay cluster the report left unmarked (no stub_path, no owned_by). No open issue names ListFileCommitHistory or revisionDag; the nearest related-but-not-covering issues are 20261023T072048Z-codecommit-10-behavioral-root-causes-three-operations-answer.md (object ids not content-addressed) and 20261102T130134Z-codecommit-content-read-and-commit-write-errors-carry.md (commit-write error wording) — neither touches the commit metadata a file-history read projects.

Cause 1 of 1: ListFileCommitHistory's revisionDag entry carries a commit author/committer date other than the date that commit was recorded with, so the FileVersion element the step expected for that commit is reported missing.

AWS: Smithy (com.amazonaws.codecommit#CodeCommit_20150413) ListFileCommitHistory returns revisionDag (@required, list of FileVersion); FileVersion.commit is the SAME Commit shape GetCommit returns (commitId, treeId, parents, message, author/committer as UserInfo{name,email,date}, additionalData), and UserInfo.date is the commit's own git identity date — unix seconds plus a GMT offset, e.g. '<unix-seconds> +0000'. A commit is immutable in git and in CodeCommit, so for one commitId every read of it (GetCommit, BatchGetCommits, ListFileCommitHistory) reports the identical author and committer date, and repeated reads report it unchanged. Nothing in the model makes a file-history projection re-derive that date. Status for this read is 200; no error is involved.

Emulator: the 200 response for the file's history is structurally right and every content-addressed field in the diverging element matches the commit the fixture had already observed — blobId 'a068def3...', commit.commitId '495bfe54...', commit.treeId '64cb8d5c...', commit.parents ['2f494561...'], message 'add docs/sub/notes.txt', author/committer name and email, path and the empty revisionChildren all agree — but commit.author.date and commit.committer.date come back as a git date OTHER than the one recorded for that commit id (replay: expected the commit's captured date, got '1794457261 +0000', the run's own clock value). Because no element of revisionDag then matches, the comparator reports $.revisionDag[0] as missing rather than a scalar mismatch. Hypothesis: the FileVersion projection does not reuse the stored commit record's identity date for this operation (or the stored date is restamped on the read path), so the date travels per read instead of per commit; the handler itself builds each entry from Commit::info() in src/ops/list_file_commit_history.rs, so the divergence is in the date that record holds or in how this projection fills it.

Why: a client that correlates a file's revision DAG with commit records it already holds — caching Commit by commitId, ordering revisions by author date, or diffing 'who changed this file when' against GetCommit output — sees two different dates for one immutable commit. Every such correlation and any date-based ordering or audit of file history is wrong against the emulator while being stable against AWS.

Fix: the ListFileCommitHistory handler's revisionDag projection and the commit record it reads the author/committer identity from (the Commit projection in the codecommit crate's state), so one commit's UserInfo.date is the date it was written with and is identical to the date GetCommit reports for the same id.

Traces: codecommit-file-folder-blob-content-operations#task#9 (ListFileCommitHistory, expected 200, actual 200, $.revisionDag[0] missing).

## Evidence
Replay cluster (divergence_key service=codecommit|ListFileCommitHistory|status=200->200|keys=$.revisionDag[0], affected_count 1, stub_path empty, owned_by empty):

  fixture codecommit-file-folder-blob-content-operations, step task#9
  expected_status 200, actual_status 200
  detail: $.revisionDag[0]: missing
    expected {'blobId': 'a068def323c4ff9b39c29215807e7e754cbb2a8a', 'commit': {'additionalData': '', 'author': {'date': '{{commit2_date}}', 'email': 'emulator', 'name': 'emulator'}, 'commitId': '495bfe547f1180ad05b438f66a5b76002721e55b', 'committer': {'date': '{{commit2_date}}', 'email': 'emulator', 'name': 'emulator'}, 'message': 'add docs/sub/notes.txt', 'parents': ['2f4945618b7df1231c40afb3b2ec67f009232edc'], 'treeId': '64cb8d5c528c09ea4b302424ee4f1c8e75d7c0aa'}, 'path': 'docs/sub/notes.txt', 'revisionChildren': []}
    got      {'commit': {'commitId': '495bfe547f1180ad05b438f66a5b76002721e55b', 'treeId': '64cb8d5c528c09ea4b302424ee4f1c8e75d7c0aa', 'parents': ['2f4945618b7df1231c40afb3b2ec67f009232edc'], 'message': 'add docs/sub/notes.txt', 'author': {'name': 'emulator', 'email': 'emulator', 'date': '1794457261 +0000'}, 'committer': {'name': 'emulator', 'email': 'emulator', 'date': '1794457261 +0000'}, 'additionalData': ''}, 'blobId': 'a068def323c4ff9b39c29215807e7e754cbb2a8a', 'path': 'docs/sub/notes.txt', 'revisionChildren': []}

The expectation is bound to THIS run, not to a stale recording: the commit id, tree id, blob id and parent id in the expected element are exactly the ids this emulator minted during the replay, and only the author/committer date field diverges. The requirement is therefore the date's identity with the commit, not any particular number: the fixed part is the git date form '<unix-seconds> <±hhmm>', and the variable part is the stored identity date of the commit named by commitId (the same value GetCommit reports for that id) — not the instant the file-history read was served.

Source read while triaging (citation only, no patch): services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/list_file_commit_history.rs:364-380 builds each FileVersion from commit.info(); services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:1692-1702 (Commit::info) and :4169-4179 (identity_info) project the stored author/committer identity; :3853-3859 (git_date) is the form the date is written in.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Fixture codecommit-file-folder-blob-content-operations, step task#9: after the fixture has created the repository and written docs/sub/notes.txt (commit 495bfe54..., child of 2f494561...), call ListFileCommitHistory with that repositoryName and filePath 'docs/sub/notes.txt'.
3. Read revisionDag[0].commit.author.date and .committer.date, and compare them with the author/committer date GetCommit returns for the same commitId (and with the date the fixture captured for that commit when it was written).
4. Observed: the history entry reports a different git date (the read's own clock value) while the ids, message, name, email, path and revisionChildren all match. Expected: the same date the commit itself carries, identical on every read of that commit id.
