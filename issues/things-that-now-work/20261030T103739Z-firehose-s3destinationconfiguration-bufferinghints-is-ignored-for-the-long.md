Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.buffering-hints-delay-delivery-to-s3
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.buffering-hints-delay-delivery-to-s3|run#12|Timeout|time.sleep(min(10, early_deadline - time.monotonic()))
Issue-Commit: e63ec425c23513ba26a0bb6d325f047894e211cc

# firehose: S3DestinationConfiguration.BufferingHints is ignored for the long-interval case — a single PutRecord is flushed to S3 immediately instead of being held for IntervalInSeconds (1 cause)

> _Note: this issue was identified against the code base at commit `e63ec425c23513ba26a0bb6d325f047894e211cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One net-new root cause in the single-service firehose suite. Every other firehose failure in this run is marked OWNED by an open issue (20261022T064119Z, 20261025T083505Z, 20261026T235333Z, 20261028T075033Z, 20261028T150444Z, 20261029T175617Z-firehose-6-net-new-root-causes-bufferinghints-are.md) or ALREADY FILED against an unimplemented stub, and is excluded here.

=== Cause 1 of 1: a delivery stream created with S3DestinationConfiguration.BufferingHints {SizeInMBs: 128, IntervalInSeconds: 120} does not hold the record for the buffering window — the object is written to the destination bucket at once, and the check never reaches its post-interval assertion ===
AWS: Smithy: com.amazonaws.firehose#BufferingHints.IntervalInSeconds @range 0..900, documented "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination. The default value is 300"; BufferingHints.SizeInMBs @range 1..128, documented "Buffer incoming data to the specified size, in MiBs, before delivering it to the destination. The default value is 5." S3DestinationConfiguration.BufferingHints carries that shape. AWS behavior for this configuration: with SizeInMBs deliberately set to the 128 MiB maximum so the size threshold cannot fire, a single ~15-byte PutRecord is NOT present in the destination bucket while the interval is still running — ListObjectsV2 on the bucket returns no Contents — and the object appears only after IntervalInSeconds has elapsed, at which point its body contains the put bytes. The interval, not the record count, is what makes the object appear.
Emulator: BufferingHints is not consulted by the S3 delivery path at all; the record is written as its own object in the same second as the PutRecord, so the destination is non-empty for the entire buffering window. The replayed check is killed by the suite's per-check cap (`Error: Check timed out after 60.0s`) before it can list the bucket, because its own early-listing wait runs to t = IntervalInSeconds - 20 = 100 s; the immediate-flush behavior itself is directly observable in the sibling checks firehose.buffering-hint-interval-delays-delivery and firehose.buffering-hint-size-triggers-early-flush, which replay the same configuration with a shorter interval and find the object (Size 14 / Size 16) already listed right after the put. This long-interval variant is the 120 s / 128 MiB corner of the same defect and is not carried by any open issue's replay keys.
Why: a client cannot rely on Firehose to coalesce records into one object per buffering window, and cannot assert "nothing has been delivered yet" during the window. Against the emulator it sees one object per record, arriving instantly, so object counts, per-window key layout, downstream batch sizes and any freshness/latency assumption are all wrong; a pipeline tuned on the emulator will behave completely differently on AWS. A long interval also makes the divergence maximal: AWS withholds the data for two minutes, the emulator for zero seconds.
Fix: the Firehose S3 delivery worker's flush scheduler — the per-stream buffer must retain records until the stream's effective BufferingHints.IntervalInSeconds elapses or BufferingHints.SizeInMBs is reached (defaults 300 s / 5 MiB when the member is absent), instead of writing an object on every PutRecord. The same scheduler must apply to the whole modelled range (IntervalInSeconds up to 900, SizeInMBs up to 128), not only to small values. Related open issues 20261029T175617Z-firehose-6-net-new-root-causes-bufferinghints-are.md (Cause 1) and 20261025T083505Z-firehose-s3-delivery-ignores-bufferinghints-the-destination-role.md describe the same family; neither carries this check's replay key.
Traces: firehose.buffering-hints-delay-delivery-to-s3.

## Evidence
Check: firehose.buffering-hints-delay-delivery-to-s3 (verdict: mismatch)
  Error: Check timed out after 60.0s

Expected (AWS): after CreateDeliveryStream(DirectPut, S3DestinationConfiguration{BucketARN, RoleARN, BufferingHints{SizeInMBs: 128, IntervalInSeconds: 120}}) reaches ACTIVE and one ~15-byte PutRecord, ListObjectsV2 on the destination bucket returns NO Contents at any point before t=120 s; after the interval elapses exactly one object appears and its body contains the put bytes.

Actual (emulator): the object is written in the same second as the PutRecord, so the bucket is non-empty throughout the window. Observed directly in the two sibling checks of the same family, which share this code path and are not killed by the cap:
  firehose.buffering-hint-interval-delays-delivery:
    AssertionError: record delivered before buffer interval elapsed: [{'Key': '<YYYY/MM/DD/HH>/<stream>-1-<ts>-0-<uuid>', 'Size': 14, ...}]
  firehose.buffering-hint-size-triggers-early-flush:
    AssertionError: single below-threshold record was flushed early: [{'Key': '...', 'Size': 16, ...}]
(The keys, sizes, timestamps and uuids above are this environment's per-run values, not requirements.)

Note for the fixer about this check's budget: the check sleeps until t = interval_s - 20 = 100 s before its first listing, so under the suite's 60 s per-check cap it is reported as a timeout rather than as the AssertionError the early listing would raise. Honouring BufferingHints is what the check asserts; confirm the fix with the two sibling checks named above (same scheduler, shorter interval) in addition to re-replaying this one.

No error code or HTTP status is involved: CreateDeliveryStream and PutRecord both answer 200 and are correct here; the divergence is purely the timing of the destination write.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. CreateBucket, and create an IAM role trusted by firehose.amazonaws.com with s3:PutObject/s3:GetBucketLocation/s3:ListBucket on that bucket.
3. CreateDeliveryStream(DeliveryStreamName=<name>, S3DestinationConfiguration={RoleARN: <role>, BucketARN: arn:aws:s3:::<bucket>, BufferingHints: {SizeInMBs: 128, IntervalInSeconds: 120}}); poll DescribeDeliveryStream until DeliveryStreamStatus == ACTIVE.
4. PutRecord(DeliveryStreamName=<name>, Record={Data: b"delayed-record\n"}).
5. Immediately, and again at any point before 120 s have elapsed, call ListObjectsV2(Bucket=<bucket>): the emulator already returns one object; AWS returns no Contents until the 120 s interval elapses.
6. After the interval elapses, list again: AWS now returns exactly one object whose body contains "delayed-record".
7. Same configuration with a shorter IntervalInSeconds (60 s) reproduces the identical immediate flush inside the suite's 60 s cap — see firehose.buffering-hint-interval-delays-delivery.
