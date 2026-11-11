Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.firehose-role-missing-s3-permission-denied events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket
Issue-Key: events__firehose__s3.firehose-role-missing-s3-permission-denied|_poll_until#17|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket|_poll_until#18|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# events__firehose__s3: a Firehose delivery role that cannot write the S3 destination produces no S3.AccessDenied record on the stream's CloudWatchLoggingOptions log stream

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
One net-new root cause under the events__firehose__s3 heading, grounded in two failing checks.

Cause 1 of 1: a delivery stream whose S3DestinationConfiguration.RoleARN cannot write the destination bucket emits no delivery-error record to the log stream named by CloudWatchLoggingOptions, so the authorization failure is invisible.

AWS: Smithy (firehose) CloudWatchLoggingOptions{Enabled, LogGroupName (length 0..512, pattern ^[\.\-_/#A-Za-z0-9]*$), LogStreamName} — 'Describes the Amazon CloudWatch logging options for your Firehose stream'; S3DestinationConfiguration.RoleARN is the role Firehose assumes to write to the bucket. The log record's own shape is not modelled, so its content is recorded behavior only: when the assumed role cannot PutObject to the destination, Firehose writes a delivery-error record into the configured log group's 'DestinationDelivery' stream whose errorCode is the fixed token 'S3.AccessDenied' (the errorMessage names the bucket/prefix from the stream's own configuration), and no object is ever written under the stream's Prefix. The same holds when the role's s3 statement is scoped to another bucket: the stream delivering to the in-scope bucket succeeds while the out-of-scope one logs S3.AccessDenied. Widening the role's policy makes the identical PutRecord deliver, which is what distinguishes a permission-specific denial from a wedged stream.

Emulator: no record with any errorCode ever appears in the broken stream's log group — the checks poll the log group for a delivery-error record and get None for the whole window (180 s in the missing-permission case, 300 s in the scoped-bucket case), while the positive-control stream with a correctly scoped role delivers its object to S3 in the same run. So either the role is never evaluated for the S3 write, or the failure is never surfaced through CloudWatchLoggingOptions; from the client's side both read as 'nothing happened'.

Why: the single observable AWS gives a client for a mis-scoped Firehose delivery role is that log record. With it absent, a broken stream is indistinguishable from an idle one: dashboards and alarms keyed on errorCode S3.AccessDenied never fire, and operators have no signal to repair the role.

Fix: the Firehose S3 delivery path's authorization of the assumed RoleARN plus the CloudWatchLoggingOptions emitter that must write the DestinationDelivery error record for a failed put.

Related open issues that do NOT carry these replay keys: 20261104T055008Z-firehose-logs-s3-cloudwatchloggingoptions-never-emits-a-delivery.md (same family under the firehose__logs__s3 heading) and 20261029T175617Z-firehose-6-net-new-root-causes-bufferinghints-are.md (its 'delivery role is never authorized for the S3 destination' cause, under the firehose heading).

Traces: events__firehose__s3.firehose-role-missing-s3-permission-denied; events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket.

## Evidence
events__firehose__s3.firehose-role-missing-s3-permission-denied (line 76): the poll for an error code in the broken stream's log group
  AssertionError: condition not met within 180.0s (last=None)
  — the expected next assertion was `logged == "S3.AccessDenied"`. The positive control (same bucket, role with s3 write + logs write) had already delivered an object under good/, so the run's Firehose→S3 path works; only the denial and its log record are missing.

events__firehose__s3.firehose-role-scoped-bucket-denied-on-other-bucket (line 70):
  AssertionError: condition not met within 300.0s (last=None)
  — same poll, for a role whose s3 statement covers bucket A only while the stream writes bucket B.

Expected vs actual:
  expected: a DestinationDelivery log record with errorCode = "S3.AccessDenied" (fixed token) within the stream's buffering interval, and no object under the broken stream's Prefix; after the role is widened, the same PutRecord lands an object.
  actual:   no log record at all (None) for the whole poll window.

## Steps to Reproduce
1. CreateBucket; CreateLogGroup.
2. CreateRole trusting firehose.amazonaws.com whose only statement is logs:PutLogEvents on that log group (no s3 action at all).
3. CreateDeliveryStream (DirectPut) with S3DestinationConfiguration {BucketARN, RoleARN = that role, Prefix 'data/', BufferingHints {SizeInMBs 1, IntervalInSeconds 60}, CloudWatchLoggingOptions {Enabled true, LogGroupName, LogStreamName 'DestinationDelivery'}}.
4. PutRecord one record. In parallel create a sibling stream with a role granting s3 write to the same bucket under prefix 'good/' and PutRecord — it must deliver (positive control).
5. Poll the broken stream's log group for a delivery-error record.
   Expected: a record with errorCode 'S3.AccessDenied'; ListObjectsV2 under 'data/' stays empty.
   Emulator: no record is ever written to the log group.
6. PutRolePolicy granting s3:PutObject on the bucket to the SAME role and PutRecord again — the record must now land under 'data/'.
7. Repeat with one role whose s3 statement is scoped to bucket A while the stream's BucketARN is bucket B (scoped-bucket variant).
