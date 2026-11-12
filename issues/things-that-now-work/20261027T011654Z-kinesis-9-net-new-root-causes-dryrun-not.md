Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.dry-run-writes-and-reads-refuse-without-mutating-the-shard kinesis.put-record-nonexistent-stream-fails kinesis.get-shard-iterator-nonexistent-stream-fails kinesis.list-shards-stream-creation-timestamp-mismatch-rejected kinesis.list-shards-streamname-and-streamarn-mismatch-rejected kinesis.create-on-demand-stream-with-shard-count-must-fail kinesis.put-record-explicit-hash-key-out-of-range-rejected kinesis.create-channel-invalid-name-fails kinesis.tag-resource-malformed-arn-rejected kinesis.update-stream-mode-invalid-mode-value-fails kinesis.put-record-partition-key-too-long-fails kinesis.stop-stream-encryption-on-unencrypted-stream-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.dry-run-writes-and-reads-refuse-without-mutating-the-shard|run#10|AssertionError|assert refused == _dry_run_refusal("PutRecord"), (
Issue-Key: kinesis.put-record-nonexistent-stream-fails|run#9|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis.get-shard-iterator-nonexistent-stream-fails|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.list-shards-stream-creation-timestamp-mismatch-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.list-shards-streamname-and-streamarn-mismatch-rejected|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.create-on-demand-stream-with-shard-count-must-fail|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.put-record-explicit-hash-key-out-of-range-rejected|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: kinesis.create-channel-invalid-name-fails|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kinesis.tag-resource-malformed-arn-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis.update-stream-mode-invalid-mode-value-fails|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: kinesis|PutRecord|ValidationException
Issue-Key: kinesis.stop-stream-encryption-on-unencrypted-stream-rejected|run#6|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# kinesis: 9 net-new root causes — DryRun, not-found, name/ARN-mismatch, on-demand ShardCount and ExplicitHashKey refusal messages are invented, the ValidationException renderer echoes anchored model patterns / model enum order / a truncated value, and StopStreamEncryption is still a 501 stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
Nine net-new root causes in the kinesis emulator (Cause 1 of 9 … Cause 9 of 9), all grounded in failing `kinesis` behavioral checks. An HTTP status, an error code and an error message are requirements, so each cause below quotes the diverging message and names which parts are fixed text and which are values taken from the request or from emulator state. Causes already covered by the suite's scaffold-stub issues (CreateChannel, SplitShard, MergeShards, UpdateShardCount, UpdateStreamMode, IncreaseStreamRetentionPeriod, StartStreamEncryption, …) are not repeated; Cause 9 is the ONE stub with no issue filed for it.

=== Cause 1 of 9: the DryRun refusal message is invented ===
Failing check: kinesis.dry-run-writes-and-reads-refuse-without-mutating-the-shard.
AWS: Smithy declares DryRunOperationException (HTTP 400) on PutRecord, PutRecords, GetShardIterator and GetRecords, each of which models a `DryRun` input member ("Checks if your request will succeed"). Recorded behavior: the refusal message is `DryRunOperation validation succeeded while calling <Operation> operation.: Request would have succeeded, but DryRun flag is set.` — fixed text apart from `<Operation>`, which is the called operation's name (PutRecord / PutRecords / GetShardIterator / GetRecords).
Emulator: the code is right but the message is a single operation-independent string: `The request was rejected because the DryRun parameter was specified.`
Why: DryRun is used as a pre-flight permission/validity probe; clients and operators match on the message to tell "would have succeeded" apart from a real refusal, and the emulator's text neither says that nor names the operation.
Fix: the shared DryRun short-circuit used by the four data-plane handlers must build the message from the operation name in AWS's wording.

=== Cause 2 of 9: stream-not-found messages use one invented template for three different refusals ===
Failing checks: kinesis.put-record-nonexistent-stream-fails, kinesis.get-shard-iterator-nonexistent-stream-fails, kinesis.list-shards-stream-creation-timestamp-mismatch-rejected.
AWS (ResourceNotFoundException, HTTP 400, modeled on all three operations) words each case differently:
  * PutRecord against a missing stream: `Stream <streamName> under account <accountId> not found.` — fixed text `Stream `, ` under account `, ` not found.`; variables = the requested stream name and the caller's account id.
  * GetShardIterator: `Shard <shardId> in stream <streamName> under account <accountId> does not exist` — the refusal is phrased at the SHARD, with the requested ShardId, stream name and account id as variables.
  * ListShards with a StreamCreationTimestamp that does not match: `Stream <streamName> under account <accountId> with creation time <timestamp> not found.` — variables = stream name, account id and the supplied creation timestamp (ISO-8601, as sent).
Emulator: all three answer the same invented lower-case template `stream <streamName> not found under account <accountId>` — word order reversed, no trailing period, no shard clause, no creation-time clause.
Why: the messages are the only thing distinguishing "no such stream" from "no such shard" and from "a different stream of the same name"; consumers and operators that match on them cannot tell which condition they hit.
Fix: the per-operation not-found construction in the PutRecord, GetShardIterator and ListShards handlers (and the shared stream-lookup helper they call) must emit the three AWS templates, filled from the request and from state.

=== Cause 3 of 9: ListShards' StreamName/StreamARN disagreement message is invented ===
Failing check: kinesis.list-shards-streamname-and-streamarn-mismatch-rejected.
AWS: when both StreamName and StreamARN are supplied and the ARN's inferred name differs, ListShards refuses HTTP 400 `InvalidArgumentException` with `Input stream name <streamName> doesn't match the inferred name in stream arn <nameFromArn>` — fixed text apart from the two names, taken from the request's StreamName and from the stream/<name> segment of the supplied StreamARN.
Emulator: `StreamName and StreamARN name different streams; name one stream or supply only one of them` — invented wording, and it misstates the contract (AWS accepts the pair when it agrees; see the paging cause in the companion issue).
Why: a client cannot see WHICH of its two identifiers was wrong, and the advice to "supply only one" is not AWS's rule.
Fix: the identifier-reconciliation step shared by the ListShards handler (and the other operations accepting both spellings) must emit AWS's message.

=== Cause 4 of 9: CreateStream's ON_DEMAND+ShardCount refusal message is invented ===
Failing check: kinesis.create-on-demand-stream-with-shard-count-must-fail.
AWS: CreateStream with StreamModeDetails.StreamMode=ON_DEMAND and a ShardCount is refused HTTP 400 `InvalidArgumentException` with the fixed message `ShardCount cannot be set while creating stream in On-Demand StreamMode` (no variable parts).
Emulator: right code, invented message `ShardCount cannot be specified for a ON_DEMAND stream; its shard count is managed by the service`.
Why: this is the canonical first-run error for on-demand streams; tooling and tests match its text.
Fix: the CreateStream handler's stream-mode validation message.

=== Cause 5 of 9: PutRecord validates ExplicitHashKey as a shard lookup instead of a range check ===
Failing check: kinesis.put-record-explicit-hash-key-out-of-range-rejected.
AWS: Smithy types ExplicitHashKey as `HashKey` with pattern `^(0|([1-9]\\d{0,38}))$`, which 2^128 (39 digits) satisfies, so the model does NOT catch it; the service range-checks the value and refuses HTTP 400 `InvalidArgumentException` with `Invalid ExplicitHashKey. ExplicitHashKey must be in the range: [0, 2^128-1]. Specified value was <value>` — fixed text apart from `<value>`, echoed from the request.
Emulator: same code, but it reports a shard-lookup failure instead: `the hash key <value> lies in no open shard of the stream; an ExplicitHashKey must fall inside one shard's HashKeyRange`. The value is out of the 128-bit key space entirely, which is a range violation, not a shard-coverage one — and the emulator's framing would also mis-describe a legitimately uncovered (but in-range) key.
Why: a producer cannot tell an out-of-range key from a resharding gap, which are different bugs on the client side.
Fix: a range check on ExplicitHashKey in the PutRecord/PutRecords handler validation, ahead of the shard lookup, with AWS's message.

=== Cause 6 of 9: ValidationException pattern messages echo the anchored Smithy pattern ===
Failing checks: kinesis.create-channel-invalid-name-fails, kinesis.tag-resource-malformed-arn-rejected.
AWS: the front-end validator renders a pattern constraint UNANCHORED: `1 validation error detected: Value at 'channelName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+` for CreateChannel's channelName (recorded behavior: the offending VALUE is omitted for this member), and `1 validation error detected: Value '<resourceARN>' at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:.*(stream|channel|queue)/\\S+` for TagResource (value echoed; fixed text otherwise).
Emulator: it prints the model pattern verbatim, anchors included, and its ARN pattern is the model's older alternation: `...pattern: ^[a-zA-Z0-9_.-]+$` (plus the value, which AWS omits for channelName) and `...pattern: ^arn:aws.*:kinesis:.*:\\d{12}:.*(stream|channel)/\\S+$` (no `queue`). The Smithy model in this repo does carry the anchored, queue-less patterns (ChannelName, ResourceARN), so the renderer is faithfully reproducing the model where AWS reproduces its service-side pattern.
Why: these strings are compared character-for-character by SDK tests and by operators diagnosing a rejected ARN; and the missing `queue` alternative tells a client that a valid resource class is invalid.
Fix: the shared constraint-violation renderer (the validation layer that builds ValidationException messages from modeled pattern constraints) must strip the `^`/`$` anchors, follow AWS's per-member rule on echoing the value, and the ResourceARN pattern it reports must include the `queue` alternative.

=== Cause 7 of 9: enum ValidationException messages list the model's member order, not AWS's ===
Failing check: kinesis.update-stream-mode-invalid-mode-value-fails.
AWS: `1 validation error detected: Value 'BOGUS_MODE' at 'streamModeDetails.streamMode' failed to satisfy constraint: Member must satisfy enum value set: [ON_DEMAND, PROVISIONED]` — fixed text apart from the rejected value; the value set is rendered in AWS's order, ON_DEMAND first.
Emulator: `… enum value set: [PROVISIONED, ON_DEMAND]` — the StreamMode enum's declaration order in the Smithy model (values: PROVISIONED, ON_DEMAND).
Why: same character-for-character comparison as above; the member path and value are right, only the set's rendering diverges.
Fix: the enum-constraint branch of the same ValidationException renderer.

=== Cause 8 of 9: length-constraint ValidationException truncates the offending value ===
Failing check: kinesis.put-record-partition-key-too-long-fails.
AWS: `1 validation error detected: Value '<the full offending value>' at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256` (Smithy: PartitionKey @length 1..256) — the value is echoed in FULL, with no annotation.
Emulator: it clips the value and appends a byte count — `Value 'kkkk…kkk... [257 bytes]' at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256` — so the message differs from AWS's for any over-limit value.
Why: the echoed value is how a producer identifies which key it sent; the `... [N bytes]` annotation is emulator-specific text no AWS client expects.
Fix: the length-constraint branch of the ValidationException renderer must echo the value unmodified.

=== Cause 9 of 9: StopStreamEncryption is an unimplemented 501 scaffold stub (no issue filed for it) ===
Failing check: kinesis.stop-stream-encryption-on-unencrypted-stream-rejected.
AWS: StopStreamEncryption on a stream whose EncryptionType is NONE is refused HTTP 400 `InvalidArgumentException` (Smithy: StopStreamEncryption declares AccessDenied/InvalidArgument/LimitExceeded/ResourceInUse/ResourceNotFound and returns an empty body on success, setting the stream to UPDATING then ACTIVE); the refused request must not start an update, and the same call succeeds once the stream really is encrypted.
Emulator: the operation answers HTTP 501 `NotImplemented` ("operation StopStreamEncryption is scaffolded but not implemented in this emulator"), so neither the refusal nor the success path exists. Unlike the other kinesis stubs this run hit, no scaffold-stub issue was filed for this operation — the suite filed issues for StartStreamEncryption and the rest, but nothing names StopStreamEncryption.
Why: encryption cannot be turned off at all, and the refusal class an unencrypted stream must produce is unreachable.
Fix: implement the handler in services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/stop_stream_encryption.rs (currently returning `Err(crate::scaffolded::Scaffolded::not_implemented("StopStreamEncryption"))` at line 206, with the SCAFFOLDED marker at line 4), including the InvalidArgumentException refusal on an already-unencrypted stream and the UPDATING→ACTIVE transition that leaves EncryptionType NONE.

## Evidence
All evidence is from the replayed behavioral suite (simparity-behavioral-ga-prod-v1), 25/231 passed. Each line is expected-vs-actual as the check reported it.

Cause 1 — AssertionError: PutRecord with DryRun=True must be refused with ('DryRunOperationException', 'DryRunOperation validation succeeded while calling PutRecord operation.: Request would have succeeded, but DryRun flag is set.'), got ('DryRunOperationException', 'The request was rejected because the DryRun parameter was specified.')

Cause 2 — AssertionError: expected message 'Stream <name> under account 000000000000 not found.', got 'stream <name> not found under account 000000000000' (PutRecord); AssertionError: expected message 'Shard shardId-000000000000 in stream <name> under account 000000000000 does not exist', got 'stream <name> not found under account 000000000000' (GetShardIterator); AssertionError: expected message 'Stream <name> under account 000000000000 with creation time 2026-10-26T01:02:59Z not found.', got 'stream <name> not found under account 000000000000' (ListShards + StreamCreationTimestamp).

Cause 3 — AssertionError: expected message "Input stream name <nameA> doesn't match the inferred name in stream arn <nameB>", got 'StreamName and StreamARN name different streams; name one stream or supply only one of them'.

Cause 4 — AssertionError: expected message 'ShardCount cannot be set while creating stream in On-Demand StreamMode', got 'ShardCount cannot be specified for a ON_DEMAND stream; its shard count is managed by the service'.

Cause 5 — AssertionError: expected message 'Invalid ExplicitHashKey. ExplicitHashKey must be in the range: [0, 2^128-1]. Specified value was 340282366920938463463374607431768211456', got "the hash key 340282366920938463463374607431768211456 lies in no open shard of the stream; an ExplicitHashKey must fall inside one shard's HashKeyRange".

Cause 6 — AssertionError: expected message "1 validation error detected: Value at 'channelName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+", got "1 validation error detected: Value 'bad channel name!!' at 'channelName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"; and expected "… pattern: arn:aws.*:kinesis:.*:\\d{12}:.*(stream|channel|queue)/\\S+", got "… pattern: ^arn:aws.*:kinesis:.*:\\d{12}:.*(stream|channel)/\\S+$" for resourceARN 'arn:aws:kinesis:us-west-2:000000000000:stream/'.

Cause 7 — AssertionError: expected "… enum value set: [ON_DEMAND, PROVISIONED]", got "… enum value set: [PROVISIONED, ON_DEMAND]".

Cause 8 — AssertionError: PutRecord over-limit PartitionKey message "1 validation error detected: Value 'kkkk…' [truncated by the emulator and annotated '... [257 bytes]'] at 'partitionKey' failed to satisfy constraint: Member must have length less than or equal to 256", expected the same message with the full 257-character value echoed and no annotation.

Cause 9 — AssertionError: expected InvalidArgumentException stopping encryption on an unencrypted stream, got NotImplemented. Source: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/stop_stream_encryption.rs:4 (SCAFFOLDED marker) and :206 (`Err(crate::scaffolded::Scaffolded::not_implemented("StopStreamEncryption"))`).

## Steps to Reproduce
Common setup: kinesis.create_stream(StreamName=n, ShardCount=1, StreamModeDetails={'StreamMode':'PROVISIONED'}), poll describe_stream_summary until ACTIVE.
Cause 1: kinesis.put_record(StreamName=n, Data=b'x', PartitionKey='pk', DryRun=True) and the same with put_records / get_shard_iterator / get_records → DryRunOperationException with the emulator's single generic message instead of "DryRunOperation validation succeeded while calling <Operation> operation.: Request would have succeeded, but DryRun flag is set."
Cause 2: put_record(StreamName='<never-created>', …); get_shard_iterator(StreamName='<never-created>', ShardId='shardId-000000000000', ShardIteratorType='TRIM_HORIZON'); list_shards(StreamName=n, StreamCreationTimestamp=<a wrong timestamp>) → all three return the same 'stream <name> not found under account <acct>' text.
Cause 3: list_shards(StreamName=n+'-a', StreamARN=<arn of n+'-b'>) → InvalidArgumentException with the emulator's invented wording.
Cause 4: create_stream(StreamName=n2, ShardCount=1, StreamModeDetails={'StreamMode':'ON_DEMAND'}) → InvalidArgumentException with the wrong message.
Cause 5: put_record(StreamName=n, Data=b'x', PartitionKey='pk', ExplicitHashKey='340282366920938463463374607431768211456') → InvalidArgumentException with the shard-coverage message instead of the range message.
Cause 6: create_channel(ChannelName='bad channel name!!', …) and tag_resource(ResourceARN='arn:aws:kinesis:us-west-2:<acct>:stream/', Tags={'k':'v'}) with botocore parameter validation disabled → ValidationException messages carrying the anchored model patterns.
Cause 7: update_stream_mode(StreamARN=<arn>, StreamModeDetails={'StreamMode':'BOGUS_MODE'}) with parameter validation disabled → enum set rendered [PROVISIONED, ON_DEMAND].
Cause 8: put_record(StreamName=n, Data=b'x', PartitionKey='k'*257) with parameter validation disabled → the value in the message is clipped and annotated '... [257 bytes]'.
Cause 9: on a stream whose EncryptionType is NONE, stop_stream_encryption(StreamName=n, EncryptionType='KMS', KeyId='alias/aws/kinesis') → HTTP 501 NotImplemented instead of HTTP 400 InvalidArgumentException.
