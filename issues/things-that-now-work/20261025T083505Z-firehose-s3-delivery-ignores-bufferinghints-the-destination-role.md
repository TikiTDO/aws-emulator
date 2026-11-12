Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-role-without-s3-permissions-denied firehose.server-side-role-permission-removal-blocks-delivery firehose.server-side-enforces-single-active-role-after-update-destination firehose.s3-buffering-hint-controls-delivery-latency firehose.s3-delivery-triggered-by-buffer-interval-timeout firehose.server-side-managed-flush-without-explicit-trigger firehose.delivered-object-content-type-and-encoding-match-declared-compression firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-role-without-s3-permissions-denied|run#26|AssertionError|assert noperm_objs == [], (
Issue-Key: firehose.server-side-role-permission-removal-blocks-delivery|run#15|AssertionError|assert "after-revoke" not in seen, "record was delivered to S3 despite the role losing all S3 permissions"
Issue-Key: firehose.server-side-enforces-single-active-role-after-update-destination|run#21|AssertionError|assert not any(marker_c in line for line in lines_c), (
Issue-Key: firehose.s3-buffering-hint-controls-delivery-latency|run#22|AssertionError|assert not slow_objects_at_checkpoint, (
Issue-Key: firehose.s3-delivery-triggered-by-buffer-interval-timeout|run#10|AssertionError|assert early_keys == [], f"record delivered before the buffer interval elapsed: {early_keys}"
Issue-Key: firehose.server-side-managed-flush-without-explicit-trigger|run#12|AssertionError|assert not seen, f"data delivered before the buffer interval elapsed, with no explicit trigger: {seen}"
Issue-Key: firehose.delivered-object-content-type-and-encoding-match-declared-compression|run#11|AssertionError|assert actual_encoding == expected_encoding, (
Issue-Key: firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion|run#29|AssertionError|assert delta == _ZONE_OFFSET, (
Issue-Key: firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe|_parse_key#19|AssertionError|assert match is not None, (
Issue-Commit: 8a7d0174a619a438b7db3f8bae6f70b79955a1d5

# Firehose S3 delivery ignores BufferingHints, the destination role's permissions, CompressionFormat Content-Encoding, CustomTimeZone and the documented object-name suffix (5 causes)

> _Note: this issue was identified against the code base at commit `8a7d0174a619a438b7db3f8bae6f70b79955a1d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
FIVE net-new root causes in the Firehose ExtendedS3 delivery path. All five are in the firehose service and are grounded in failing behavioral checks that now get PAST the "no object is ever written" stage: the emulator does materialise objects in the destination bucket, and each of these nine checks now fails on an assertion about HOW that object was produced (an AssertionError at the check's own assertion frame), not on a destination poll timing out.

Relationship to open issues: 20261024T005347Z-firehose-never-delivers-buffered-records-to-the-s3.md and 20261025T062017Z-firehose-directput-delivery-to-an-extendeds3-destination-never.md both describe the single cause "a buffered record never becomes an S3 object" and carry these check ids with a Timeout-at-a-sleep replay key. That is not what the replay shows now — objects appear (keys and ETags are quoted below) — and the replay report does not mark any of the nine checks below as OWNED by those issues. The five causes here are the behaviors that become observable once an object IS written, and each needs its own fix; none of them is the flush-never-runs cause those issues carry. 20261022T194351Z-firehose-control-plane-configuration-echo-drops-members-tag.md and 20261022T064119Z-firehose-createdeliverystream-putrecordbatch-are-overly-permissive-and-misclassify.md cover the CONTROL plane (echo, tag/KMS validation, refusal classification) and cover none of these.

=== Cause 1 of 5: delivery does not evaluate the destination role's IAM permissions, so a role with no s3:PutObject still delivers ===
Failing checks: firehose.create-stream-role-without-s3-permissions-denied, firehose.server-side-role-permission-removal-blocks-delivery, firehose.server-side-enforces-single-active-role-after-update-destination.
AWS: Smithy ExtendedS3DestinationConfiguration.RoleARN is @required, length 1..512, pattern ^arn:.*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$, documented as "the Amazon Resource Name (ARN) of the Amazon Web Services credentials"; Firehose assumes exactly that role to write to BucketARN. CreateDeliveryStream therefore succeeds on a syntactically valid role, and the missing-permission boundary is only visible in the data plane: with no s3:PutObject on the destination bucket, Firehose cannot write and no object appears under the configured Prefix (the failure surfaces only as a delivery error, e.g. through CloudWatchLoggingOptions). The same applies after the fact: revoking the role's S3 permissions stops subsequent deliveries, and after UpdateDestination only the NEW RoleARN is the active identity, so a replacement role without s3:PutObject stops delivery even though the previously configured role had it.
Emulator: delivery writes the object unconditionally. The stream wired to a role deliberately created WITHOUT s3:PutObject delivered anyway (object 'noperm/2026/10/25/08/sim2real-fh-noperm-…' with a 16-byte body); a role stripped of all S3 permissions after creation kept delivering ('after-revoke' marker found in S3); and after UpdateDestination swapped in role B with no S3 permissions the record was still delivered. The emulator's positive controls pass, so it is not that delivery is skipped — authorization is simply never consulted.
Why: a client cannot discover a misconfigured delivery role. Against real AWS a stream with an under-privileged role silently delivers nothing (and reports a delivery error); against the emulator every such stream looks healthy, so IAM-regression tests, least-privilege rollouts and break-glass revocations all report the opposite of production. It also makes the "single active role after UpdateDestination" invariant untestable.
Fix: the delivery/flush component of the Firehose service crate must resolve the destination's current RoleARN against the IAM service's state and evaluate s3:PutObject on the destination bucket/prefix before writing, failing the delivery (not the put) when it is not allowed; it must read the role from the CURRENT destination configuration so UpdateDestination's role replaces the old one.
Traces: the three check ids above.

=== Cause 2 of 5: BufferingHints are ignored — every put is flushed to S3 immediately ===
Failing checks: firehose.s3-buffering-hint-controls-delivery-latency, firehose.s3-delivery-triggered-by-buffer-interval-timeout, firehose.server-side-managed-flush-without-explicit-trigger.
AWS: Smithy BufferingHints.IntervalInSeconds (@range 0..900, default 300) is "Buffer incoming data for the specified period of time, in seconds, before delivering it to the destination" and BufferingHints.SizeInMBs (@range 1..128, default 5) is "Buffer incoming data to the specified size, in MiBs, before delivering it to the destination". Firehose delivers when one of the two thresholds is reached, so with IntervalInSeconds=120 and a payload far below SizeInMBs no object exists in the destination bucket within the first seconds after the put; a stream configured with a short interval delivers measurably sooner than one with a long interval.
Emulator: the object is present within ~10 s regardless of the configured interval. "slow stream (interval=120s) delivered to S3 within 10s, same as the fast stream -- buffering interval had no effect"; "record delivered before the buffer interval elapsed: ['2026/10/25/08/sim2real-behav-fh-stream-…']"; and all ten markers of the auto-flush leg ('auto-flush-0'..'auto-flush-9') were readable before the interval elapsed. The hints are accepted and echoed but do not gate the flush.
Why: latency and batching are the properties Firehose exists for. A pipeline tuned on the emulator sees per-record objects at zero latency, so object counts, object sizes, downstream small-file costs and any test that asserts "not yet delivered" invert in production; conversely a test that asserts data IS delivered only after the interval can never catch a regression.
Fix: the flush scheduler in the Firehose service crate must hold accepted records until the destination's BufferingHints.SizeInMBs or IntervalInSeconds threshold is met (applying the modeled defaults 5 MiB / 300 s when absent) and only then write one object per flushed batch.
Traces: the three check ids above.

=== Cause 3 of 5: the delivered object carries no Content-Encoding for a compressed CompressionFormat ===
Failing check: firehose.delivered-object-content-type-and-encoding-match-declared-compression.
AWS: Smithy CompressionFormat is an enum UNCOMPRESSED|GZIP|ZIP|Snappy|HADOOP_SNAPPY, default UNCOMPRESSED, on ExtendedS3DestinationConfiguration.CompressionFormat ("The compression format. If no value is specified, the default is UNCOMPRESSED"). Recorded behavior (the check states it was measured on real AWS): the delivered object has ContentType=application/octet-stream and a ContentEncoding naming the declared format in lower case — "gzip" for GZIP and "zip" for ZIP — while UNCOMPRESSED carries no ContentEncoding at all.
Emulator: the GZIP leg's delivered object has NO ContentEncoding: "leg 'ctenc-gzip' (CompressionFormat='GZIP') delivered ContentEncoding None, expected 'gzip'". ContentType is correct, and the UNCOMPRESSED leg passes, so the object metadata is written but the compression format is not reflected into it (the ZIP leg is unreached because the check stops at the first failing leg).
Why: clients and SDK/browser transports key off Content-Encoding to decompress transparently; an object whose bytes are gzip but whose metadata says nothing is handed to consumers as opaque bytes, and tooling that inspects the header to choose a decoder (or that distinguishes "zip" from "gzip") reads the emulator as UNCOMPRESSED.
Fix: the Firehose delivery writer must set the object's Content-Encoding from the destination's CompressionFormat when it compresses (gzip for GZIP, zip for ZIP, and the corresponding value for Snappy/HADOOP_SNAPPY), leaving it unset for UNCOMPRESSED, while keeping Content-Type application/octet-stream.
Traces: the check id above.

=== Cause 4 of 5: CustomTimeZone has no effect on the expanded timestamp prefix ===
Failing check: firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion.
AWS: Smithy ExtendedS3DestinationConfiguration.CustomTimeZone, length 0..50, pattern ^$|[a-zA-Z/_]+$, doc "The time zone you prefer. UTC is the default." Prefix is documented as "The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3 files" and supports !{timestamp:...} expressions. So the timestamp expansion is performed in the configured zone: two streams fed the same instant, one defaulted to UTC and one with CustomTimeZone='Asia/Tokyo', expand the same !{timestamp:...} pattern to values offset by that zone's UTC offset (Asia/Tokyo is UTC+09:00 year round). Hypothesis (the check labels it so): the same zone also moves the object-name suffix timestamp.
Emulator: the member is accepted but ignored — both legs expanded to the identical instant: "CustomTimeZone='Asia/Tokyo' did not shift the expanded timestamp prefix: the UTC-default leg expanded to 2026-10-25T08:00:00+00:00 (key 'tz-utc/d=2026-10-25/h=08/…') and the Asia/Tokyo leg to 2026-10-25T08:00:00+00:00 (key 'tz-tokyo/d=2026-10-25/h=08/…'), a delta of 0:00:00, expected exactly 9:00:00".
Why: partition layout is a contract with the query layer. A table partitioned on the local-time prefix lands its rows in the wrong partition on the emulator, so date-ranged queries and lifecycle rules built against local-day boundaries select the wrong objects, and the whole point of setting CustomTimeZone is unverifiable.
Fix: the prefix-expansion component of the Firehose delivery path must evaluate !{timestamp:...} (and the automatic YYYY/MM/DD/HH prefix) in the destination's CustomTimeZone, defaulting to UTC when it is absent or empty.
Traces: the check id above.

=== Cause 5 of 5: the delivered object name does not match the documented default format — an extra numeric segment is inserted before the UUID ===
Failing check: firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe.
AWS: the documented default object name is <evaluated prefix><stream name>-<stream version>-yyyy-MM-dd-HH-mm-ss-<uuid><file extension> (docs.aws.amazon.com/firehose/latest/dev/s3-object-name.html), i.e. under the automatic YYYY/MM/DD/HH prefix the tail is exactly stream, version, a single second-resolution timestamp and the UUID — nothing between the timestamp and the UUID. The version segment is the decimal stream version that DescribeDeliveryStream reports as VersionId and that increments on each configuration change.
Emulator: an extra numeric segment is written between the timestamp and the UUID, so the name does not parse: "delivered key '2026/10/25/08/sim2real-fh-ver-08fb188088e1-1-2026-10-25-08-21-04-0-0d1339a8-5c09-4a13-9f1d-c5b9d2b0065f' does not match the documented default object name format 'YYYY/MM/dd/HH/<stream>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid>'" — the tail is <stream>-1-2026-10-25-08-21-04-0-<uuid>, with a '-0' (an apparent batch/sequence counter) that AWS does not emit. The same shape appears in every other delivered key in this run (e.g. 'noperm/2026/10/25/08/sim2real-fh-noperm-b5da146fb498-1-2026-10-25-08-22-38-0-a7942661-…'), so it is systematic, not per-check. Because the name does not parse, the downstream assertion that the version segment tracks DescribeDeliveryStream's VersionId across a configuration change cannot be evaluated at all.
Why: consumers parse these names. Glue/Athena partition projection, log shippers, de-duplication by object name and anything deriving stream/version/time from the key break on the extra segment, and a client cannot tell which configuration version produced an object.
Fix: the object-key builder in the Firehose delivery path must emit <evaluated prefix><stream name>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid><file extension> with the stream's current version and no additional segment; the per-object uniqueness belongs in the UUID.
Traces: the check id above.

## Evidence
Replayed suite: simparity-behavioral-ga-prod-v1 (firehose 97/139). Nine failing firehose.* checks, none marked ALREADY FILED and none marked OWNED by an open issue.

Cause 1 — role permissions not enforced at delivery
Expected (AWS): a delivery stream whose ExtendedS3 RoleARN lacks s3:PutObject on the destination bucket writes NO object (list_objects_v2 returns no Contents), while the sibling stream with a permitted role does; revoking the role's S3 permissions stops later deliveries; after UpdateDestination only the new RoleARN is the active identity.
Actual (emulator): firehose.create-stream-role-without-s3-permissions-denied — "AssertionError: stream wired to a role WITHOUT s3:PutObject delivered data anyway -- found objects: [{'Key': 'noperm/2026/10/25/08/sim2real-fh-noperm-b5da146fb498-1-2026-10-25-08-22-38-0-a7942661-b943-4377-8793-745452464a35', 'Size': 16, 'ETag': '\"a4a170db9e04c7fa62d92c23bbaf5b2f\"'}]". firehose.server-side-role-permission-removal-blocks-delivery — "AssertionError: record was delivered to S3 despite the role losing all S3 permissions". firehose.server-side-enforces-single-active-role-after-update-destination — "AssertionError: record was delivered despite the currently-active role (B) having no S3 permissions".

Cause 2 — BufferingHints ignored
Expected (AWS): with BufferingHints.IntervalInSeconds set (120 s / 60 s in these checks) and a payload far below SizeInMBs, the destination bucket has no object before the interval elapses, and a long-interval stream delivers measurably later than a short-interval one.
Actual (emulator): firehose.s3-buffering-hint-controls-delivery-latency — "AssertionError: slow stream (interval=120s) delivered to S3 within 10s, same as the fast stream -- buffering interval had no effect". firehose.s3-delivery-triggered-by-buffer-interval-timeout — "AssertionError: record delivered before the buffer interval elapsed: ['2026/10/25/08/sim2real-behav-fh-stream-99b08b2f9cb9-1-2026-10-25-08-06-38-0-60a89edd-3ce9-4c47-a255-6f403e3b8e83']". firehose.server-side-managed-flush-without-explicit-trigger — "AssertionError: data delivered before the buffer interval elapsed, with no explicit trigger: {'auto-flush-0' … 'auto-flush-9'}".

Cause 3 — Content-Encoding absent for compressed formats
Expected (AWS, measured): ContentType=application/octet-stream for every leg; ContentEncoding='gzip' for CompressionFormat=GZIP, 'zip' for ZIP, absent for UNCOMPRESSED.
Actual (emulator): firehose.delivered-object-content-type-and-encoding-match-declared-compression — "AssertionError: leg 'ctenc-gzip' (CompressionFormat='GZIP') delivered ContentEncoding None, expected 'gzip'" (ContentType assertion for that leg passed; the UNCOMPRESSED leg passed).

Cause 4 — CustomTimeZone ignored
Expected (AWS): with CustomTimeZone='Asia/Tokyo' (UTC+09:00, no DST) the treated leg's expanded !{timestamp:...} prefix is exactly 9 h ahead of the UTC-default leg's for the same instant.
Actual (emulator): firehose.custom-time-zone-shifts-the-timestamp-prefix-expansion — "AssertionError: CustomTimeZone='Asia/Tokyo' did not shift the expanded timestamp prefix: the UTC-default leg expanded to 2026-10-25T08:00:00+00:00 (key 'tz-utc/d=2026-10-25/h=08/…') and the Asia/Tokyo leg to 2026-10-25T08:00:00+00:00 (key 'tz-tokyo/d=2026-10-25/h=08/…'), a delta of 0:00:00, expected exactly 9:00:00".

Cause 5 — object name has an extra segment
Expected (AWS): key matches ^(YYYY/MM/DD/HH)/<stream>-<version>-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}-\d{2}-<uuid>$ with the version equal to DescribeDeliveryStream's VersionId at the time of the put.
Actual (emulator): firehose.delivered-object-suffix-carries-the-stream-version-reported-by-describe — "AssertionError: delivered key '2026/10/25/08/sim2real-fh-ver-08fb188088e1-1-2026-10-25-08-21-04-0-0d1339a8-5c09-4a13-9f1d-c5b9d2b0065f' does not match the documented default object name format 'YYYY/MM/dd/HH/<stream>-<version>-yyyy-MM-dd-HH-mm-ss-<uuid>'" — an extra '-0' sits between the timestamp and the UUID; the same shape appears in every delivered key in this run, so the version-tracking assertion behind it is unreachable.

## Steps to Reproduce
Common setup: boot the emulator, point botocore at it, s3.create_bucket(Bucket=B).

Cause 1 (role permissions):
1. iam.create_role(R_no) with a trust policy for firehose.amazonaws.com and NO s3:PutObject; iam.create_role(R_ok) same trust policy plus s3:PutObject/s3:AbortMultipartUpload on arn:aws:s3:::B and B/*.
2. firehose.create_delivery_stream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={'RoleARN': R_no, 'BucketARN': 'arn:aws:s3:::B', 'Prefix': 'noperm/', 'BufferingHints': {'SizeInMBs': 1, 'IntervalInSeconds': 60}}) and a sibling stream with R_ok and Prefix 'permok/'.
3. Wait for both ACTIVE, put one record into each, then poll s3.list_objects_v2(Bucket=B).
4. Expect objects only under 'permok/'; observe objects under 'noperm/' too.
5. Variant A: create one stream with R_ok, deliver successfully, then remove all S3 permissions from R_ok and put another record — expect no new object, observe one. Variant B: update_destination to a role B with no S3 permissions and put a record — expect no new object, observe one.

Cause 2 (BufferingHints):
1. Create two DirectPut streams on B, one with BufferingHints={'SizeInMBs':1,'IntervalInSeconds':120} and one with a short interval; wait for ACTIVE.
2. Put one small record into each and poll s3.list_objects_v2 every second for 10 s.
3. Expect the 120 s stream to have no object yet; observe its object within ~10 s, same as the fast stream. Same for a single stream with IntervalInSeconds=60: expect nothing before 60 s.

Cause 3 (Content-Encoding):
1. Create one stream per leg with CompressionFormat UNCOMPRESSED / GZIP / ZIP and distinct prefixes; wait for ACTIVE; put the same payload into each.
2. For each delivered key, s3.get_object and read ContentType/ContentEncoding.
3. Expect application/octet-stream everywhere and ContentEncoding 'gzip'/'zip' for the compressed legs; observe ContentEncoding missing (None) on the GZIP leg.

Cause 4 (CustomTimeZone):
1. Create two streams on B with Prefix 'tz-utc/d=!{timestamp:yyyy-MM-dd}/h=!{timestamp:HH}/' and 'tz-tokyo/d=!{timestamp:yyyy-MM-dd}/h=!{timestamp:HH}/', the second with CustomTimeZone='Asia/Tokyo'.
2. Put one record into each at the same instant and read back the delivered keys.
3. Expect the two expanded timestamps to differ by exactly 9 h; observe an identical expansion (delta 0).

Cause 5 (object name):
1. Create a DirectPut stream S on B with no Prefix, wait for ACTIVE, note describe_delivery_stream(...)['DeliveryStreamDescription']['VersionId'].
2. Put one record and read the delivered key.
3. Expect 'YYYY/MM/DD/HH/S-<VersionId>-yyyy-MM-dd-HH-mm-ss-<uuid>'; observe an extra '-<n>' segment between the timestamp and the uuid.
4. update_destination once (a real configuration change), put another record, and check the new key's version segment equals the bumped VersionId.

Finally re-replay the nine check ids in trace_ids.
