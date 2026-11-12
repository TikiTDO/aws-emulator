Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3
Issue-Key: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3|_warm_up_delivery#9|AssertionError|raise AssertionError(
Issue-Commit: c16b2b764bbf06a743836a4d21d50909208bc2e5

# events__firehose__s3: an event routed through an EventBridge rule's Firehose target is never readable in the S3 destination within the buffering window — every record ages for its own full interval instead of riding the buffer's flush

> _Note: this issue was identified against the code base at commit `c16b2b764bbf06a743836a4d21d50909208bc2e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
=== Cause 1 of 1: the Firehose S3 flush does not carry records that arrived while the current buffer was already filling, so no freshly put event is ever readable within the buffering window ===
Failing check: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3.

Relationship to open issues: 20261109T135435Z-events-firehose-s3-an-eventbridge-rule-with-a.md records the older symptom that an EventBridge rule's Firehose target delivered NOTHING at all; that is no longer what happens here — the envelopes do land in the bucket, correctly shaped, so this is a different cause and the replay marks this check neither ALREADY FILED nor OWNED. 20261109T135435Z-firehose-3-net-new-causes-bufferinghints-intervalinseconds-is.md Cause 1 reports the opposite extreme (IntervalInSeconds ignored, every put written immediately) on plain firehose.* checks; neither issue carries this check's replay key, and the behavior observed here is neither "immediate" nor "nothing".

AWS: Smithy BufferingHints documents IntervalInSeconds (@range 0..900, default 300) as "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination" and SizeInMBs (@range 1..128, default 5) as the size threshold; the structure doc adds that Firehose delivers when one of the two thresholds is reached. The interval is a property of the BUFFER, anchored to the moment the buffer became non-empty, not of each individual record: when the interval elapses, the flush writes one object containing EVERY record accumulated since the previous flush, including a record that arrived only seconds before the boundary. EventBridge's part of the contract: PutEvents on a bus whose rule EventPattern matches calls firehose:PutRecord with the event envelope under the target's RoleArn, and the record then follows the stream's own buffering contract. Consequently, under a steady trickle of one small event every ~20 s into a stream configured IntervalInSeconds=60 / SizeInMBs=1, flushes occur roughly once per interval and each flush sweeps up the most recent arrivals, so within a couple of intervals some event is readable in the destination bucket within ~20 s of its own PutEvents. Exact per-record latency is recorded behavior only: the recording requires that, within a 180 s window of repeated single-event PutEvents calls spaced 20 s apart, at least ONE of those events becomes readable out of the destination bucket within 20 s of the call that produced it.

Emulator: across the full 180 s window (≈9 consecutive single-event PutEvents attempts, each followed by 20 s of polling list_objects_v2 + get_object on the destination bucket) not one event was readable within 20 s of its own PutEvents — yet by the time the window expired, EVERY earlier event was present in the bucket and parsed correctly. The final observation lists the delivered envelopes for the whole preceding sequence (`detail-type` 'sim2real.test.warmup', the rule's `source`, `detail.marker` for each successive attempt, `account`, `region`, `resources: []`), i.e. the rule matched, the target was invoked, PutRecord landed and the S3 object body is the concatenated-JSON EventBridge envelope AWS writes. Only the flush timing diverges: a record never appears in the object written at the end of the interval that was already running when it arrived; it only appears a full interval after its own arrival.

Hypothesis: the flush scheduler tracks the IntervalInSeconds deadline per record (or restarts the stream's pending-flush timer on each arrival) rather than anchoring it to the first record of the current buffer, so a record put into an already-filling buffer cannot ride the pending flush and instead waits out another whole interval. Nothing in the model or the recording states the mechanism; the observable requirement is the buffer-anchored flush above.

Why: a client that trusts the documented hint budgets end-to-end latency as "at most IntervalInSeconds after my put" and sizes alarms, pollers and downstream batch windows on it. On the emulator the worst-case latency is unbounded by the configured interval under continuous traffic, and the object a flush writes omits the newest records it should contain — so object boundaries, record-per-object counts and any freshness SLO measured against the emulator all understate what production does. End-to-end EventBridge→Firehose→S3 tests that wait one buffering interval before asserting become flaky-to-impossible even though the wiring and payload are right.

Fix: the buffering/flush scheduler of the Firehose delivery path (the component that owns the destination's BufferingHints for the S3/ExtendedS3 destination) must anchor the IntervalInSeconds deadline to the first record in the current buffer, and on flush emit one object containing every record accepted since the previous flush — including records accepted while that interval was running — applying the modeled defaults (5 MiB / 300 s) when the hints are absent. The EventBridge rule-dispatch side is not implicated: it already calls PutRecord with the correct envelope.

Traces: events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3 (the assertion fires in the check's bounded warm-up loop, before the batch-of-three and default-key-layout assertions can be evaluated, so those two contracts are also unverified behind this cause).

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — events__firehose__s3 19/26 (73%), 1 failing check that the replay marks neither ALREADY FILED nor OWNED (6 further checks in this group were unloadable and are not evidence of anything).

Setup performed by the check, all of which succeeded: s3.create_bucket; iam.create_role trusted by firehose.amazonaws.com with s3:PutObject/GetBucketLocation/ListBucket/AbortMultipartUpload/GetObject on the bucket; firehose.create_delivery_stream(DeliveryStreamType='DirectPut', S3DestinationConfiguration={'RoleARN': R_fh, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}) reaching DeliveryStreamStatus ACTIVE; iam.create_role trusted by events.amazonaws.com with firehose:PutRecord on the stream ARN; events.put_rule(EventPattern={'source':[<src>]}, State='ENABLED'); events.put_targets(...) returning FailedEntryCount 0.

Expected (AWS): within the 180 s warm-up window, at least one of the single-event PutEvents calls (one every 20 s) has its event readable out of the destination bucket within 20 s of that call, because the flush at the end of the running 60 s interval includes the records that arrived during it.

Actual (emulator): "AssertionError: warmup event never delivered to S3 via Firehose within 180.0s." — no attempt's own marker was ever visible in its 20 s poll window. The same assertion's final observation shows the earlier events DID land, each as a well-formed EventBridge envelope read back out of a delivered S3 object, one per successive attempt and spaced at the attempt interval: entries with 'detail-type': 'sim2real.test.warmup', the rule's 'source', 'resources': [], and 'detail': {'marker': <that attempt's marker>} for attempt 1, attempt 2, attempt 3, ... (the per-run marker strings, event ids, account/region and 'time' values in that dump are environment values, not requirements — the requirement is that an event put at T is readable by T + IntervalInSeconds at the latest, and that the object written when the running interval expires contains the records accepted during it).

So: control plane correct, rule match correct, target invocation correct, record payload and envelope correct, destination object body correct — only the flush schedule diverges.

## Steps to Reproduce
1. Boot the emulator and point botocore at it. s3.create_bucket(Bucket=B).
2. iam.create_role(R_fh) trusted by firehose.amazonaws.com with s3:PutObject/s3:AbortMultipartUpload/s3:GetBucketLocation/s3:ListBucket/s3:GetObject on arn:aws:s3:::B and arn:aws:s3:::B/*.
3. firehose.create_delivery_stream(DeliveryStreamName=S, DeliveryStreamType='DirectPut', S3DestinationConfiguration={'RoleARN': R_fh, 'BucketARN': 'arn:aws:s3:::B', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}); poll describe_delivery_stream until DeliveryStreamStatus == 'ACTIVE'.
4. iam.create_role(R_evt) trusted by events.amazonaws.com with firehose:PutRecord/firehose:PutRecordBatch on S's ARN.
5. events.put_rule(Name=RU, EventPattern='{"source":["<SRC>"]}', State='ENABLED'); events.put_targets(Rule=RU, Targets=[{'Id':'firehose-target','Arn':<S arn>,'RoleArn':R_evt}]) and confirm FailedEntryCount == 0.
6. In a loop for up to 180 s: pick a fresh marker M, call events.put_events(Entries=[{'Source':'<SRC>','DetailType':'t','Detail': json.dumps({'marker': M})}]), then for the next 20 s poll s3.list_objects_v2(Bucket=B) and get_object each key, parsing the body as concatenated JSON, looking for an envelope whose detail.marker == M.
7. Expect at least one iteration to find its own marker within its 20 s window (the flush at the end of the already-running 60 s interval carries it). Observe that no iteration ever does, while the bucket steadily accumulates the envelopes of ALL previous iterations — each record only surfaces about a full interval after its own arrival, never in the flush that was already pending when it arrived.
8. Equivalent direct-Firehose reproduction without EventBridge: on the same stream, put_record one small record at t=0, a second at t=50 (interval 60 s), and list the bucket at t=65. Expect one object containing BOTH records; observe the t=50 record missing from it.

Finally re-replay events__firehose__s3.eventbridge-put-events-triggers-firehose-to-s3.
