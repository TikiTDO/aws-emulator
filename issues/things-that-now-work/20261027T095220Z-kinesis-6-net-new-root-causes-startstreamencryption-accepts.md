Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected kinesis.increase-retention-below-current-fails kinesis.retention-period-limits-record-visibility-window kinesis.kms-encrypted-stream-returns-plaintext-records kinesis.merge-non-adjacent-shards-must-fail kinesis.update-stream-mode-concurrent-conflict-resource-in-use kinesis.update-stream-mode-while-resharding-blocked kinesis.update-stream-mode-while-updating-conflicts
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.increase-retention-below-current-fails|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis.retention-period-limits-record-visibility-window|run#23|AssertionError|assert message == expected_message, (
Issue-Key: kinesis.kms-encrypted-stream-returns-plaintext-records|run#9|AssertionError|raise AssertionError(
Issue-Key: kinesis.merge-non-adjacent-shards-must-fail|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis.update-stream-mode-concurrent-conflict-resource-in-use|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.update-stream-mode-while-resharding-blocked|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.update-stream-mode-while-updating-conflicts|run#11|AssertionError|assert refused == expected, f"expected {expected!r}, got {refused!r}"
Issue-Commit: 27a112835af69d276912b63d12e8de98cdf07be6

# kinesis: 6 net-new root causes — StartStreamEncryption accepts a nonexistent KMS key, and the ListShards ShardFilter, closed-shard, retention-too-short, non-adjacent-merge and not-ACTIVE refusal messages are all invented

> _Note: this issue was identified against the code base at commit `27a112835af69d276912b63d12e8de98cdf07be6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartStreamEncryption, ListShards, MergeShards, IncreaseStreamRetentionPeriod, UpdateStreamMode

## Description
Six net-new root causes in the kinesis service, all reached by the emulator (each call was answered, none hit a scaffold stub). One is a wrongly ACCEPTED call that leaves false state; the other five are refusals that carry the right error code but a message the emulator invented instead of AWS's.

Related already-open issues, named here so they are not duplicated and so a fixer does not assume they cover these: `20261027T011654Z-kinesis-9-net-new-root-causes-dryrun-not.md` and `20261027T011654Z-kinesis-9-net-new-root-causes-cloudformation-aws.md` (other invented kinesis refusal messages — neither lists any of the nine check ids below), and the stub issues `20261027T011654Z-kinesis-startstreamencryption-stub.md`, `20261027T011654Z-kinesis-mergeshards-stub.md`, `20261027T011654Z-kinesis-increasestreamretentionperiod-stub.md`, `20261027T011654Z-kinesis-updatestreammode-stub.md` (those operations now answer for real on these paths, so the causes below are behind no stub). The resolved issue `20261027T025958Z-kinesis-5-net-new-root-causes-updateshardcount-s.md` graded different contracts on the same operations (the 8760-hour ceiling, the on-demand merge guard, the concurrent-update winner) and does not cover any cause here.

=== Cause 1 of 6: ListShards invents the missing-ShardFilter-Timestamp refusal message ===
AWS: Smithy ListShards (com.amazonaws.kinesis, 2013-12-02) — input member ShardFilter targets the ShardFilter structure (Type/ShardId/Timestamp); modeled errors include InvalidArgumentException (client kind, HTTP 400). Recorded behavior: ListShards with ShardFilter={Type: AT_TIMESTAMP} and no Timestamp is refused 400 InvalidArgumentException with message, fixed text exactly and in full: "Timestamp must be provided when the ShardFilter Type is AT_TIMESTAMP or FROM_TIMESTAMP." This message has NO variable parts — it names both filter types that require Timestamp, not just the one that was sent.
Emulator: the code is right (InvalidArgumentException) but the message is "ShardFilter.Timestamp is required with the AT_TIMESTAMP shard type, as a Unix epoch date" — a wholly different sentence that names only the type sent and adds an invented "Unix epoch date" clause.
Why: a client that matches on the refusal text (or a human reading it) is told the rule applies only to AT_TIMESTAMP, so code that supplies FROM_TIMESTAMP without a Timestamp is written against the wrong contract; any message assertion or log-based alarm keyed to AWS's sentence fails against the emulator.
Fix: the ShardFilter validation in the ListShards handler of the kinesis service crate's ops module.
Traces: kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage.

=== Cause 2 of 6: the already-merged-or-split shard refusal message is reworded ===
AWS: Smithy MergeShards — ShardToMerge and AdjacentShardToMerge are both @required ShardId; modeled errors include InvalidArgumentException (client, HTTP 400). Recorded behavior: merging a shard that is already CLOSED (it was previously merged or split) is refused with message, fixed text exactly: "Shard <ShardId> in stream <StreamName> under account <AccountId> has already been merged or split, and thus is not eligible for merging or splitting." Fixed text is everything but <ShardId> and <StreamName> (taken from the request) and <AccountId> (the caller's account from emulator state); the shard id, stream name and account digits in any one recording are only that run's fill-ins.
Emulator: same code and same template EXCEPT the clause "and thus is not eligible" is emitted as "so it is not eligible" — i.e. "... has already been merged or split, so it is not eligible for merging or splitting."
Why: the message is a requirement, not prose. A client or test that compares the refusal text byte-for-byte (the usual way to distinguish the closed-shard refusal from the other InvalidArgumentException refusals MergeShards can raise, which all share one code) sees a mismatch and cannot classify the failure.
Fix: the closed-shard (lineage) guard's message construction in the MergeShards handler of the kinesis service crate's ops module.
Traces: kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected.

=== Cause 3 of 6: IncreaseStreamRetentionPeriod's shorter-than-current refusal names the wrong field and the wrong API ===
AWS: Smithy IncreaseStreamRetentionPeriod — input RetentionPeriodHours (@required, integer); modeled errors include InvalidArgumentException (client, HTTP 400). Recorded behavior: a request whose RetentionPeriodHours is below the stream's current retention is refused with message, fixed text exactly: "Requested retention period (<H> hours) for stream <StreamName> can not be shorter than existing retention period (<C> hours). Use DecreaseRetentionPeriod API." Fixed text is everything but <H> (the request's RetentionPeriodHours), <StreamName> (the request) and <C> (the stream's current RetentionPeriodHours from emulator state). Note the two literal spans the emulator gets wrong: "existing retention period" and "Use DecreaseRetentionPeriod API."
Emulator: "Requested retention period (<H> hours) for stream <StreamName> can not be shorter than existing value (<C> hours). Use DecreaseStreamRetentionPeriod API." — "existing retention period" became "existing value" and the remediation hint names a different API name than AWS emits.
Why: the remediation hint is the actionable part of this refusal. A client (or an operator) that reads the emulator's text is pointed at an API name AWS does not emit in this message, and any assertion or runbook built on AWS's sentence diverges. The same wording is emitted on both paths that compare requested against current retention, so two independent checks fail on it.
Emulator status/code are otherwise correct (400 InvalidArgumentException).
Fix: the retention-comparison guard's message construction in the IncreaseStreamRetentionPeriod handler of the kinesis service crate's ops module.
Traces: kinesis.increase-retention-below-current-fails, kinesis.retention-period-limits-record-visibility-window.

=== Cause 4 of 6: StartStreamEncryption ACCEPTS a KeyId that does not resolve in KMS ===
AWS: Smithy StartStreamEncryption — EncryptionType and KeyId are both @required (KeyId length 1..2048); the operation models KMSNotFoundException, KMSDisabledException, KMSInvalidStateException, KMSAccessDeniedException, KMSOptInRequired, KMSThrottlingException and InvalidArgumentException, all client kind, HTTP 400. KMSNotFoundException exists precisely because AWS resolves the supplied KeyId against KMS synchronously at call time. Recorded behavior: StartStreamEncryption with a syntactically valid key ARN in the caller's own account and region that does not exist is refused 400 KMSNotFoundException with message, fixed text exactly: "Key '<KeyId>' does not exist" — fixed text is everything but <KeyId>, which is echoed from the request. The stream must stay EncryptionType=NONE.
Emulator: the call is ACCEPTED (2xx). Replay: "StartStreamEncryption ACCEPTED the nonexistent key arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000; real AWS resolves the KeyId against KMS and refuses." The stream then reports itself encrypted under a key that cannot be used.
Why: this is the only validation standing between a typo'd/stale key ARN and a stream that claims SSE it does not have. A client that calls StartStreamEncryption and gets 200 believes its data is encrypted with the key it named; against AWS it would have been told KMSNotFoundException and retried with a real key. Worse, the emulator reaches a state AWS cannot reach — ACTIVE + EncryptionType=KMS with an unresolvable KeyId — so no downstream code path that handles the KMS error families can be exercised at all.
Fix: KeyId resolution in the StartStreamEncryption handler of the kinesis service crate's ops module — resolve the KeyId against the KMS emulator's key state before mutating the stream, and refuse with KMSNotFoundException when it does not resolve, leaving the stream's encryption state untouched.
Traces: kinesis.kms-encrypted-stream-returns-plaintext-records.

=== Cause 5 of 6: the non-adjacent MergeShards refusal message is invented ===
AWS: Smithy MergeShards — ShardToMerge/AdjacentShardToMerge @required; modeled errors include InvalidArgumentException (client, HTTP 400). Recorded behavior: merging two shards that are not an adjacent pair is refused with message, fixed text exactly: "Shards <ShardA> and <ShardB> in stream <StreamName> under account <AccountId> are not an adjacent pair of shards eligible for merging" — fixed text is everything but <ShardA>/<ShardB>/<StreamName> (from the request) and <AccountId> (emulator state), and the sentence carries no trailing period.
Emulator: same code, but the message is "Shards <ShardA> and <ShardB> in stream <StreamName> under account <AccountId> are not adjacent, so the union of their hash key ranges does not form a contiguous set with no gaps." — the second half of the sentence is invented and a trailing period is added.
Why: distinct from Cause 2, which is the CLOSED-shard refusal; this is the adjacency refusal and it has its own AWS sentence. Both refusals share error code InvalidArgumentException, so the message is the only thing a client can use to tell "these two shards are not neighbours" from "this shard is already closed" — and the emulator's text matches neither AWS sentence.
Fix: the adjacency guard's message construction in the MergeShards handler of the kinesis service crate's ops module.
Traces: kinesis.merge-non-adjacent-shards-must-fail.

=== Cause 6 of 6: the not-ACTIVE ResourceInUseException message identifies the stream by ARN and rewords the state clause ===
AWS: Smithy UpdateStreamMode — StreamARN and StreamModeDetails both @required; modeled errors include ResourceInUseException (client, HTTP 400). Recorded behavior: UpdateStreamMode against a stream that is not ACTIVE (because an earlier UpdateStreamMode or a reshard is still in flight) is refused 400 ResourceInUseException with message, fixed text exactly: "Stream <StreamName> under account <AccountId> not ACTIVE, instead in state <StreamStatus>" — fixed text is everything but <StreamName> (the stream's name, NOT its ARN), <AccountId> (emulator state) and <StreamStatus> (the stream's live status from emulator state, UPDATING on these paths). Note the leading capital "Stream", the "under account <AccountId>" clause, and no trailing period.
Emulator: "stream <StreamARN> is in state <StreamStatus>, not ACTIVE; the stream must be ACTIVE for this operation" — lower-case, identifies the stream by full ARN instead of name, drops the account clause, inverts the state/ACTIVE order and appends an invented trailing clause. The error code (ResourceInUseException) and status are correct; only the message diverges, and it diverges identically on all three paths that raise it (concurrent UpdateStreamMode, UpdateStreamMode while resharding, UpdateStreamMode while an update is in flight), so this is one shared message-construction site.
Why: this refusal is the signal a client backs off and retries on, and the message is how it (and its operators) identify WHICH stream and WHICH state blocked the call. A client parsing the stream name out of AWS's sentence gets an ARN; a test asserting the AWS text fails; and the account attribution AWS provides is lost.
Fix: the not-ACTIVE / stream-state guard's message construction shared by the UpdateStreamMode handler in the kinesis service crate's ops module (and the state-guard helper it calls) — format from the stream's name plus the caller's account, not the ARN.
Traces: kinesis.update-stream-mode-concurrent-conflict-resource-in-use, kinesis.update-stream-mode-while-resharding-blocked, kinesis.update-stream-mode-while-updating-conflicts.

## Evidence
Behavioral replay, suite simparity-behavioral-ga-prod-v1 (overall 57/228; kinesis 53/196). Every check below reached the emulator and was answered — none was blocked by a scaffold stub, and none is marked ALREADY FILED or OWNED in the replay report.

1) kinesis.deep-kinesis-resharding-list-shards-timestamp-filters-resolve-split-lineage [mismatch]
   expected: 400 InvalidArgumentException, Message "Timestamp must be provided when the ShardFilter Type is AT_TIMESTAMP or FROM_TIMESTAMP." (no variable parts)
   actual:   AssertionError: expected message 'Timestamp must be provided when the ShardFilter Type is AT_TIMESTAMP or FROM_TIMESTAMP.', got 'ShardFilter.Timestamp is required with the AT_TIMESTAMP shard type, as a Unix epoch date'
   (the check separately asserted the code and it matched: InvalidArgumentException)

2) kinesis.deep-kinesis-resharding-merge-with-closed-parent-rejected [mismatch]
   expected: Message "Shard <ShardId> in stream <StreamName> under account <AccountId> has already been merged or split, and thus is not eligible for merging or splitting."
   actual:   AssertionError: expected message 'Shard shardId-000000000000 in stream <stream> under account <account> has already been merged or split, and thus is not eligible for merging or splitting.', got '... has already been merged or split, so it is not eligible for merging or splitting.' (shard id / stream name / account in the recording are that run's fill-ins)

3) kinesis.increase-retention-below-current-fails [mismatch]
   expected: Message "Requested retention period (<H> hours) for stream <StreamName> can not be shorter than existing retention period (<C> hours). Use DecreaseRetentionPeriod API."
   actual:   AssertionError: expected message 'Requested retention period (36 hours) for stream <stream> can not be shorter than existing retention period (48 hours). Use DecreaseRetentionPeriod API.', got 'Requested retention period (36 hours) for stream <stream> can not be shorter than existing value (48 hours). Use DecreaseStreamRetentionPeriod API.' (36/48 are the run's H and C)

4) kinesis.retention-period-limits-record-visibility-window [mismatch] — same message site as (3)
   actual:   AssertionError: decreasing IncreaseStreamRetentionPeriod message '... can not be shorter than existing value (48 hours). Use DecreaseStreamRetentionPeriod API.', expected '... can not be shorter than existing retention period (48 hours). Use DecreaseRetentionPeriod API.'

5) kinesis.kms-encrypted-stream-returns-plaintext-records [mismatch]
   expected: 400 KMSNotFoundException, Message "Key '<KeyId>' does not exist", stream left EncryptionType=NONE
   actual:   AssertionError: StartStreamEncryption ACCEPTED the nonexistent key arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000; real AWS resolves the KeyId against KMS and refuses. Accepting it yields a stream that reports itself encrypted with a key nobody can use

6) kinesis.merge-non-adjacent-shards-must-fail [mismatch]
   expected: Message "Shards <A> and <B> in stream <S> under account <AccountId> are not an adjacent pair of shards eligible for merging"
   actual:   AssertionError: expected message 'Shards shardId-000000000000 and shardId-000000000003 in stream <stream> under account <account> are not an adjacent pair of shards eligible for merging', got '... are not adjacent, so the union of their hash key ranges does not form a contiguous set with no gaps.'

7) kinesis.update-stream-mode-concurrent-conflict-resource-in-use [mismatch]
   expected: ResourceInUseException, Message "Stream <StreamName> under account <AccountId> not ACTIVE, instead in state UPDATING"
   actual:   AssertionError: expected message 'Stream <stream> under account <account> not ACTIVE, instead in state UPDATING', got 'stream arn:aws:kinesis:<region>:<account>:stream/<stream> is in state UPDATING, not ACTIVE; the stream must be ACTIVE for this operation'

8) kinesis.update-stream-mode-while-resharding-blocked [mismatch] — identical divergence to (7)
   actual:   AssertionError: expected message 'Stream <stream> under account <account> not ACTIVE, instead in state UPDATING', got 'stream arn:aws:kinesis:...:stream/<stream> is in state UPDATING, not ACTIVE; the stream must be ACTIVE for this operation'

9) kinesis.update-stream-mode-while-updating-conflicts [mismatch] — identical divergence to (7); code matched
   actual:   AssertionError: expected ('ResourceInUseException', 'Stream <stream> under account <account> not ACTIVE, instead in state UPDATING'), got ('ResourceInUseException', 'stream arn:aws:kinesis:...:stream/<stream> is in state UPDATING, not ACTIVE; the stream must be ACTIVE for this operation')

Model facts used (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json): ListShards input ShardFilter -> ShardFilter structure, errors AccessDenied/ExpiredNextToken/InvalidArgument/LimitExceeded/ResourceInUse/ResourceNotFound (all client, HTTP 400 defaulted). MergeShards input ShardToMerge + AdjacentShardToMerge @required ShardId, errors include InvalidArgumentException and ValidationException. IncreaseStreamRetentionPeriod input RetentionPeriodHours @required, errors include InvalidArgumentException. UpdateStreamMode input StreamARN + StreamModeDetails @required, errors include ResourceInUseException. StartStreamEncryption input EncryptionType + KeyId @required (KeyId length 1..2048), errors include KMSNotFoundException, KMSDisabledException, KMSInvalidStateException, KMSAccessDeniedException, KMSOptInRequired, KMSThrottlingException, InvalidArgumentException — all client kind, HTTP 400.

## Steps to Reproduce
1. Boot the kinesis emulator from this checkout (the KMS emulator must be reachable for cause 4).
2. Cause 1: CreateStream(ShardCount=1), wait ACTIVE, then ListShards(StreamName=..., ShardFilter={"Type": "AT_TIMESTAMP"}) with no Timestamp — observe 400 InvalidArgumentException with message 'ShardFilter.Timestamp is required with the AT_TIMESTAMP shard type, as a Unix epoch date' instead of AWS's 'Timestamp must be provided when the ShardFilter Type is AT_TIMESTAMP or FROM_TIMESTAMP.'; confirm the same filter WITH a Timestamp still resolves shards.
3. Cause 2: CreateStream(ShardCount=2, PROVISIONED), wait ACTIVE, MergeShards the two shards so the parents close, wait ACTIVE, then MergeShards again naming a now-CLOSED shard — observe '... so it is not eligible for merging or splitting.' instead of '... and thus is not eligible for merging or splitting.'
4. Cause 3: CreateStream(ShardCount=1), IncreaseStreamRetentionPeriod(RetentionPeriodHours=48) and wait ACTIVE, then IncreaseStreamRetentionPeriod(RetentionPeriodHours=36) — observe the message using 'existing value' and 'Use DecreaseStreamRetentionPeriod API.' instead of 'existing retention period' and 'Use DecreaseRetentionPeriod API.'
5. Cause 4: CreateStream(ShardCount=1), confirm DescribeStreamSummary reports EncryptionType=NONE, then StartStreamEncryption(EncryptionType=KMS, KeyId="arn:aws:kms:<same region>:<same account>:key/00000000-0000-0000-0000-000000000000") — observe a 2xx acceptance (and DescribeStreamSummary subsequently reporting EncryptionType=KMS with that KeyId) instead of 400 KMSNotFoundException "Key '<that arn>' does not exist" and the stream staying NONE.
6. Cause 5: CreateStream(ShardCount=4, PROVISIONED), wait ACTIVE, ListShards, then MergeShards(ShardToMerge=first shard, AdjacentShardToMerge=last shard) — observe the invented 'are not adjacent, so the union of their hash key ranges...' message instead of AWS's 'are not an adjacent pair of shards eligible for merging'.
7. Cause 6: CreateStream(PROVISIONED, ShardCount=1), wait ACTIVE, start an UpdateStreamMode(ON_DEMAND) (or an UpdateShardCount reshard) so the stream reads UPDATING, then issue a second UpdateStreamMode on the same StreamARN — observe ResourceInUseException with 'stream <arn> is in state UPDATING, not ACTIVE; the stream must be ACTIVE for this operation' instead of 'Stream <name> under account <account> not ACTIVE, instead in state UPDATING'.
8. Re-replay the nine check ids in trace_ids to confirm the fix.
