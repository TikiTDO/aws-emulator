Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ebs.put-snapshot-block-checksum-mismatch-rejected
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs.put-snapshot-block-checksum-mismatch-rejected|_wait_for_snapshot_state#13|Timeout|time.sleep(interval_s)
Issue-Commit: b65c74910afb27e037dd1378191061f945f503fe

# ebs: the checksum-mismatch rejection check cannot complete — an EBS direct-API snapshot is still invisible to EC2 DescribeSnapshots, so the post-CompleteSnapshot wait never converges (1 cause)

> _Note: this issue was identified against the code base at commit `b65c74910afb27e037dd1378191061f945f503fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSnapshot

## Description
REOPENS 20261029T065746Z-cloudformation-ec2-ec2-describesnapshots-answers-501-notimplemented-so.md: its fix left the call refused.

One net-new-to-this-replay-key cause, grounded in the single failing EBS check this run that is neither ALREADY FILED nor OWNED: ebs.put-snapshot-block-checksum-mismatch-rejected (3/29 ebs checks passed). Open issue 20261030T204611Z-ebs-direct-api-snapshots-are-invisible-to-ec2.md describes the SAME root cause for 21 sibling checks, but it does not carry this check's replay key (it keys this check's id nowhere; open issue 20261022T224826Z-ebs-direct-apis-ec2-describesnapshots-stub-blocks-23.md keys it as `ebs.put-snapshot-block-checksum-mismatch-rejected|run#8|AssertionError` on the checksum-class assertion, which now PASSES in this build), so this check is still unowned and is filed here; see also open issue 20261030T201123Z-ec2-describesnapshots-cannot-see-a-snapshot-created-through.md. A fixer closing this must make the check reach its final assertion, not just the checksum one.

=== Cause 1 of 1: a snapshot created by EBS StartSnapshot/CompleteSnapshot never becomes visible to EC2 DescribeSnapshots, so a client can never observe it reach `completed` ===
AWS: StartSnapshot creates a real EC2 snapshot in state `pending` and CompleteSnapshot seals it (Smithy com.amazonaws.ebs: StartSnapshot POST /snapshots code 201, output Status one of pending|completed|error; CompleteSnapshot POST /snapshots/completion/{SnapshotId} code 202, output Status; PutSnapshotBlock PUT /snapshots/{SnapshotId}/blocks/{BlockIndex} code 201, aws.auth#sigv4 with @unsignedPayload and @required Checksum). The EBS model exposes NO read of snapshot state, so EC2 DescribeSnapshots is the only way a caller observes the lifecycle: the id StartSnapshot returned resolves immediately with State `pending`, and after CompleteSnapshot it resolves with State `completed` and Progress `100%`; an id EC2 genuinely does not hold is refused with error code `InvalidSnapshot.NotFound` (recorded behavior only for the EC2 response shape). The whole point of this check is the step AFTER the refusal: once the checksum-mismatched write is refused and a correctly-checksummed block is written at another index, the snapshot must complete and ListSnapshotBlocks must then show the written index only.
Emulator: the checksum refusal itself is now correct (403 InvalidSignatureException with the computed-vs-supplied checksum message — the former cause 2 of 20261022T224826Z), but the check never gets past the completion wait: `ec2.describe_snapshots(SnapshotIds=[<id returned by StartSnapshot>])` is refused `InvalidSnapshot.NotFound` for the entire poll window, so the 60 s check cap kills the run (`Error: Check timed out after 60.0s`) inside `_wait_for_snapshot_state`, whose body is that DescribeSnapshots poll on a 3 s interval. The reason is visible in the checkout: services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs inserts the minted snapshot ONLY into the EBS crate's own store (`store.snapshots.insert(...)`, via `crate::state::service_state`); nothing registers it with the snapshot state the EC2 DescribeSnapshots/DeleteSnapshot handlers read.
Why: the EBS direct-API contract is write-then-wait-then-read. A client that calls StartSnapshot / PutSnapshotBlock / CompleteSnapshot and then polls DescribeSnapshots (or the `snapshot_completed` waiter) is told the snapshot does not exist, so it never learns the snapshot is usable, can never verify that a refused (bad-checksum) block left no residue while the good block persisted, and cannot drive the delete path — every check's cleanup calls ec2:DeleteSnapshot on an id EC2 denies, so the emulator also leaks the snapshot.
Fix: the snapshot state shared between the ebs and ec2 service crates — the EBS StartSnapshot and CompleteSnapshot handlers must register and update the snapshot in the same state the EC2 DescribeSnapshots/DeleteSnapshot handlers read, so StartSnapshot yields a `pending` snapshot resolvable by id, CompleteSnapshot moves it to `completed` with Progress `100%`, and DeleteSnapshot removes it.
Traces: ebs.put-snapshot-block-checksum-mismatch-rejected.

## Evidence
Suite: 3/29 passed (10.3%); mismatched 23, errored 3, skipped 0. Service ebs (bootstrapped). 25 of the 26 failures were marked OWNED by an open issue and are not re-filed; this issue covers the one that was not.

ebs.put-snapshot-block-checksum-mismatch-rejected [mismatch]: `Error: Check timed out after 60.0s`.
- Expected: after `start_snapshot(VolumeSize=1)`, a PutSnapshotBlock carrying the checksum of DIFFERENT bytes is refused (403 InvalidSignatureException, message 'The value passed in as x-amz-Checksum does not match the computed checksum. Computed checksum: <b64 sha256 of the bytes that arrived> expected checksum: <the Checksum header value>' — fixed text with both checksums taken from the request/payload); then a correctly-checksummed block at BlockIndex 1 succeeds, `complete_snapshot(ChangedBlocksCount=1)` is accepted, `ec2.describe_snapshots(SnapshotIds=[snap])` reports State 'completed', and `list_snapshot_blocks(SnapshotId=snap)` returns exactly BlockIndex {1} (the mismatched block must not have persisted).
- Actual: the checksum refusal is produced correctly, but `ec2.describe_snapshots(SnapshotIds=[<the id StartSnapshot returned>])` answers error code `InvalidSnapshot.NotFound` on every poll, so `_wait_for_snapshot_state` (3 s interval, 150 s budget) spins until the harness 60 s cap and the final ListSnapshotBlocks assertion is never reached.
- Code evidence for the cause: services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs lines ~388-416 build and persist the `Snapshot` record only in the EBS crate's `crate::state` store; there is no write into any EC2-visible snapshot state in that handler or in complete_snapshot.rs.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point botocore `ebs` and `ec2` clients at it.
2. `snap = ebs.start_snapshot(VolumeSize=1, Tags=[{'Key':'sim2real-purpose','Value':'<any>'}])['SnapshotId']`.
3. `ebs.put_snapshot_block(SnapshotId=snap, BlockIndex=0, BlockData=<524288 random bytes>, DataLength=524288, Checksum=<b64 sha256 of DIFFERENT 524288 bytes>, ChecksumAlgorithm='SHA256')` -> expect 403 InvalidSignatureException with the computed-vs-supplied checksum message (this part passes today).
4. `ebs.put_snapshot_block(... BlockIndex=1, Checksum=<correct b64 sha256> ...)` then `ebs.complete_snapshot(SnapshotId=snap, ChangedBlocksCount=1)`.
5. Poll `ec2.describe_snapshots(SnapshotIds=[snap])`. Expected: HTTP 200 with that snapshot, State 'pending' then 'completed' / Progress '100%'. Observed: `InvalidSnapshot.NotFound` indefinitely, so the check is killed at the 60 s cap before it can assert `ebs.list_snapshot_blocks(SnapshotId=snap)` returns block indices {1}.
6. Replay ebs.put-snapshot-block-checksum-mismatch-rejected to confirm.
