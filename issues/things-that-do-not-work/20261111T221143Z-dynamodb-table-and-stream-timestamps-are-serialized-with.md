Issue-Source: trace-sweep-divergence
Issue-Trace: create-table-duplicate-index-name-within-one-family-rejected
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-table-duplicate-index-name-within-one-family-rejected create-table-rejects-empty-secondary-index-lists dynamodb-create-table-keyschema-arity-and-order dynamodb-create-table-multi-error-ordering dynamodb-gsi-key-schema-four-hash-four-range-limit dynamodb-table-definition-list-and-map-member-scalar-rejected create-table-secondary-index-name-pattern-and-length dynamodb-update-table-gsi-update-entry-validation dynamodb-create-table-secondary-index-name-length-limits dynamodb-rbp-traj-stream-arn-has-no-table-policy
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|2xx->2xx|$.TableDescription.CreationDateTime
Issue-Key: dynamodb|DeleteTable|2xx->2xx|$.TableDescription.BillingModeSummary.LastUpdateToPayPerRequestDateTime
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.BillingModeSummary.LastUpdateToPayPerRequestDateTime
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.CreationDateTime
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime
Issue-Key: dynamodb|DescribeTable|2xx->2xx|$.Table.LatestStreamArn
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: table and stream timestamps are serialized with non-AWS precision and are re-stamped on read

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Every DynamoDB control-plane timestamp the emulator emits is shaped differently from AWS's, and one of them is additionally recomputed on every read instead of being stored. 7 causes.

Cause 1 of 7: CreateTable.TableDescription.CreationDateTime precision.
AWS: Smithy: TableDescription.CreationDateTime targets com.amazonaws.dynamodb#Date (type timestamp), documented as "the date and time when the table was created, in UNIX epoch time format". On the awsJson1_0 wire DynamoDB serializes Date as epoch seconds with at most millisecond resolution (three fractional digits).
Emulator: serializes the same member with full sub-microsecond float resolution (seven fractional digits), so the value does not have the shape AWS ever produces.
Why: a client that round-trips, hashes, formats or compares the serialized timestamp (or a recorder that normalises on AWS's precision) sees a value AWS cannot emit; SDK date parsers that assume millisecond resolution lose or mangle the trailing digits.
Fix: the DynamoDB awsJson1_0 timestamp serializer for the Date shape — truncate epoch-second doubles to millisecond resolution before writing, in the serializer rather than in each handler.
Traces: create-table-duplicate-index-name-within-one-family-rejected#task#5 (CreateTable 200, $.TableDescription.CreationDateTime).

Cause 2 of 7: DeleteTable.TableDescription.BillingModeSummary.LastUpdateToPayPerRequestDateTime precision.
AWS: same Date shape under BillingModeSummary; millisecond resolution.
Emulator: full float resolution again, on the DeleteTable response path.
Why: same as cause 1, on the delete acknowledgement a client persists as the table's final state.
Fix: same serializer; the DeleteTable handler's TableDescription projection must go through it.
Traces: create-table-duplicate-index-name-within-one-family-rejected#task#7 (DeleteTable 200).

Cause 3 of 7: DescribeTable emits three over-precise Date members at once.
AWS: $.Table.CreationDateTime, $.Table.BillingModeSummary.LastUpdateToPayPerRequestDateTime and $.Table.TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime are all Date, all millisecond-resolution.
Emulator: all three carry full float resolution.
Why: DescribeTable is the canonical read of table metadata, so every consumer of table state sees the wrong timestamp shape.
Fix: same serializer, applied to the whole TableDescription tree rather than per member.
Traces: create-table-rejects-empty-secondary-index-lists#task#6 (DescribeTable 200).

Cause 4 of 7: same defect on a DescribeTable that returns only CreationDateTime and BillingModeSummary.LastUpdateToPayPerRequestDateTime.
AWS: as cause 3.
Emulator: as cause 3.
Why: as cause 3.
Fix: as cause 3.
Traces: dynamodb-create-table-keyschema-arity-and-order#task#8 (DescribeTable 200).

Cause 5 of 7: same defect on TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime alone.
AWS: as cause 3.
Emulator: as cause 3.
Why: as cause 3.
Fix: as cause 3.
Traces: dynamodb-create-table-secondary-index-name-length-limits#task#3 (DescribeTable 200).

Cause 6 of 7: LatestStreamArn's stream label is built with two fractional digits instead of three.
AWS: Smithy: TableDescription.LatestStreamLabel is documented as "a timestamp, in ISO 8601 format, for this stream", and LatestStreamArn is arn:aws:dynamodb:<region>:<account>:table/<name>/stream/<label>. AWS always renders the label with exactly three fractional-second digits (YYYY-MM-DDTHH:MM:SS.mmm); the recorded ARN ends .778.
Emulator: rendered the label with two fractional digits (the recorded emulator value ends .04), so the ARN is structurally wrong regardless of clock. Only the digit count is the requirement here — the wall-clock instant itself legitimately differs per run.
Why: a client that parses the stream label back to a timestamp, or matches a stream ARN against the documented pattern, fails on a two-digit fraction; DynamoDB Streams consumers key off this ARN.
Fix: the stream-label formatter in the table-state/stream component — format with fixed 3-digit milliseconds, never a trimmed float.
Traces: dynamodb-rbp-traj-stream-arn-has-no-table-policy#task#4 (DescribeTable 200, $.Table.LatestStreamArn).

Cause 7 of 7: TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime is recomputed at read time instead of recording when the mode last changed.
AWS: recorded behavior: the member holds the instant the table's throughput mode was last switched to PAY_PER_REQUEST and does not move on subsequent DescribeTable calls; the recording asserts a fixed earlier value while CreationDateTime is left unasserted.
Emulator: returned the same value as CreationDateTime, i.e. the current serialization/creation instant, for the throughput-mode summary.
Why: a client polling DescribeTable to detect when a mode change settled never sees a stable "last update" instant, so change-detection and drift checks never converge.
Fix: DynamoDB table state — store the mode-change instant on the table record when the mode is set and read it back, rather than deriving it in the DescribeTable projection.
Traces: dynamodb-table-definition-list-and-map-member-scalar-rejected#task#7 (DescribeTable 200).

## Evidence
Cause 1: CreateTable 200, $.TableDescription.CreationDateTime — expected a millisecond-resolution epoch Date, got 1794434570.9029098 (7 fractional digits).
Cause 2: DeleteTable 200, $.TableDescription.BillingModeSummary.LastUpdateToPayPerRequestDateTime — same over-precise float.
Cause 3: DescribeTable 200 — $.Table.CreationDateTime, $.Table.BillingModeSummary.LastUpdateToPayPerRequestDateTime and $.Table.TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime all over-precise.
Cause 4: DescribeTable 200 — CreationDateTime + BillingModeSummary.LastUpdateToPayPerRequestDateTime over-precise.
Cause 5: DescribeTable 200 — TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime over-precise.
Cause 6: DescribeTable 200, $.Table.LatestStreamArn — recorded AWS form .../stream/<ISO8601 with 3-digit ms> (e.g. ...:29.778); emulator produced .../stream/<ISO8601 with 2-digit fraction> (e.g. ...:11.04). The requirement is the 3-digit fraction, not the instant.
Cause 7: DescribeTable 200, $.Table.TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime — expected the stored mode-change instant, got the same value as CreationDateTime (recomputed).

## Steps to Reproduce
1. Replay fixture create-table-duplicate-index-name-within-one-family-rejected and inspect the CreateTable response at task#5 and the DeleteTable response at task#7: count the fractional digits on CreationDateTime / BillingModeSummary.LastUpdateToPayPerRequestDateTime.
2. Replay create-table-rejects-empty-secondary-index-lists#task#6, dynamodb-create-table-keyschema-arity-and-order#task#8 and dynamodb-create-table-secondary-index-name-length-limits#task#3 and do the same on the DescribeTable $.Table timestamps.
3. Replay dynamodb-rbp-traj-stream-arn-has-no-table-policy#task#4 and read $.Table.LatestStreamArn: the stream label after the final '/' must carry exactly three fractional-second digits.
4. Replay dynamodb-table-definition-list-and-map-member-scalar-rejected#task#7: TableThroughputModeSummary.LastUpdateToPayPerRequestDateTime must be the stored mode-change instant, not the table's creation/serialization instant.
