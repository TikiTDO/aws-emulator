Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-esm-maximum-record-age-60-discards-aged-records-delivers-fresh
Issue-Key: dynamodb__lambda.ddb-stream-esm-maximum-record-age-60-discards-aged-records-delivers-fresh|run#24|AssertionError|assert subject_seen == {_FRESH}, (
Issue-Commit: 1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30

# dynamodb__lambda: a DynamoDB-stream ESM ignores MaximumRecordAgeInSeconds — records older than the filter are delivered instead of discarded (1 cause)

> _Note: this issue was identified against the code base at commit `1acf76f902f2311eb0ad0e51c71d77c4cf2a9a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: the DynamoDB-Streams event-source poller applies no record-age filter, so `MaximumRecordAgeInSeconds` is a no-op and an age-filtered mapping behaves exactly like an infinite-age (-1) one.

AWS: Smithy (lambda) models `com.amazonaws.lambda#CreateEventSourceMappingRequest$MaximumRecordAgeInSeconds` -> `com.amazonaws.lambda#MaximumRecordAgeInSeconds`, integer @range(min=-1, max=604800), documented "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than the specified age. The default value is infinite (-1)." The model does not state the delivery timing, so for the delivery itself this is recorded behavior only: the recording runs two otherwise identical TRIM_HORIZON stream ESMs over twin stream-enabled tables that were handed the same backlog in the same instant. The control mapping (MaximumRecordAgeInSeconds=-1) delivers the whole aged backlog AND a record written after the mapping existed. The subject mapping (MaximumRecordAgeInSeconds=60, attached once the shard's oldest record's own `dynamodb.ApproximateCreationDateTime` is demonstrably older than 60 s) delivers ONLY the record written after the mapping existed: every record whose ApproximateCreationDateTime is older than the configured age is dropped by the poller, the shard keeps advancing past them, the mapping stays State 'Enabled', and GetEventSourceMapping echoes MaximumRecordAgeInSeconds=60. All of this is consumer-side: a strongly-consistent GetItem on each source table still returns the last written version. The age boundary is the general requirement — age(record) > MaximumRecordAgeInSeconds => not delivered, -1 => never age out — not any one run's version numbers.

Emulator: both legs delivered the identical set. The subject mapping with MaximumRecordAgeInSeconds=60 delivered the aged backlog as well as the fresh record, i.e. the same record set and the same window as its age=-1 control twin, so the configured age had no observable effect whatsoever. Replay output: "AssertionError: a DynamoDB-stream ESM with MaximumRecordAgeInSeconds=60 must discard the records already older than that and deliver the fresh one: the aged-out mapping saw [0, 1, 2, 3, 99], expected [99] (control saw [0, 1, 2, 3, 99])" — the per-run values there are the handler-written version numbers; the requirement is that the subject's delivered set excludes every record older than the filter while still containing the post-attachment record. No error status or error code is involved: every call in the flow succeeds with 200, the divergence is purely in which records the poller hands to the function. (Note: unlike the earlier filings on this wiring, delivery itself now works on BOTH legs here — the control leg delivers correctly — so this is specifically a missing age filter, not a missing delivery path.)

Why: a client relying on AWS's behavior loses the only DynamoDB-stream knob that bounds a shard's progress by wall-clock age. Backlog shedding and poison/stale-batch containment silently do nothing: a consumer that sets a maximum age so an outage-era backlog is skipped instead receives the full stale backlog and reprocesses it, and no observable (State, LastProcessingResult, record set) distinguishes a working filter from an ignored one. Conversely, a test suite that asserts the aged records are absent cannot pass against the emulator.

Fix: the DynamoDB-Streams event-source-mapping poller behind lambda CreateEventSourceMapping — the per-record age filter derived from the mapping's MaximumRecordAgeInSeconds, compared against each record's ApproximateCreationDateTime, with -1 treated as infinite, and the shard checkpoint advanced past discarded records so the mapping keeps progressing and stays Enabled.

Related open issues named per instructions; neither carries this check's replay key, and both were filed for the DIFFERENT check id `ddb-stream-esm-maximum-record-age-discards-aged-records` (this one is `...-maximum-record-age-60-discards-aged-records-delivers-fresh`), so neither covers it: 20261109T172142Z-dynamodb-lambda-4-net-new-causes-esm-maximumrecordageinseconds.md (its Cause 1 describes the same ignored age filter) and 20261026T090231Z-dynamodb-lambda-a-dynamodb-streams-event-source-mapping.md (filed when neither leg delivered at all).

Traces: dynamodb__lambda.ddb-stream-esm-maximum-record-age-60-discards-aged-records-delivers-fresh.

## Evidence
Check dynamodb__lambda.ddb-stream-esm-maximum-record-age-60-discards-aged-records-delivers-fresh — verdict: mismatch.
  AssertionError at check line 163: a DynamoDB-stream ESM with MaximumRecordAgeInSeconds=60 must discard the records already older than that and deliver the fresh one: the aged-out mapping saw [0, 1, 2, 3, 99], expected [99] (control saw [0, 1, 2, 3, 99])

Expected (recorded, real AWS):
  - control mapping (MaximumRecordAgeInSeconds=-1): delivered set = every aged record written before the mapping existed PLUS the record written after it.
  - subject mapping (MaximumRecordAgeInSeconds=60, attached only after the shard's oldest record reported an ApproximateCreationDateTime more than 60 s in the past): delivered set = the post-attachment record ONLY; no record older than 60 s is ever handed to the function.
  - GetEventSourceMapping(UUID=<subject>) -> State in ('Enabled','Updating'), MaximumRecordAgeInSeconds == 60.
  - GetItem(ConsistentRead=True) on both source tables -> the last written version; the age filter never touches table state.

Actual (emulator):
  - subject mapping delivered the aged backlog as well as the fresh record — set identical to the control's. The check's earlier control assertion passed, so the delivery path works and only the age filter is missing.

Smithy cited: com.amazonaws.lambda#CreateEventSourceMappingRequest$MaximumRecordAgeInSeconds -> com.amazonaws.lambda#MaximumRecordAgeInSeconds, integer @range(min=-1, max=604800); doc "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than the specified age. The default value is infinite (-1)."

## Steps to Reproduce
1. CreateTable an audit table; PutItem {pk:'log', entries:{L:[]}}.
2. CreateTable two twin source tables with StreamSpecification {StreamEnabled:true, StreamViewType:'NEW_AND_OLD_IMAGES'}, BillingMode PAY_PER_REQUEST, key pk (S); keep each LatestStreamArn.
3. CreateFunction (python3.12) with a role allowing dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams on <source arn>/stream/* and dynamodb:UpdateItem on the audit table; the handler appends {src:<table from eventSourceARN>, b:[NewImage.v ...]} to the audit item. Wait for State Active.
4. Before any mapping exists, PutItem v=0 then UpdateItem v=1,2,3 on BOTH source tables.
5. Poll dynamodbstreams DescribeStream + GetShardIterator(TRIM_HORIZON) + GetRecords on the subject stream until the oldest record's dynamodb.ApproximateCreationDateTime is >= 70 s in the past (demonstrably older than the filter).
6. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<subject stream arn>, Enabled=true, StartingPosition='TRIM_HORIZON', BatchSize=9, MaximumBatchingWindowInSeconds=0, MaximumRecordAgeInSeconds=60); the same on the control stream with MaximumRecordAgeInSeconds=-1.
7. UpdateItem v=99 on both source tables.
8. Poll GetItem(audit table, {pk:'log'}, ConsistentRead=True): the control leg accumulates {0,1,2,3,99}; AWS's subject leg accumulates {99} only. Against the emulator the subject leg also accumulates {0,1,2,3,99} — the aged records were not discarded.
9. GetEventSourceMapping(UUID=<subject>) must still report State 'Enabled' and MaximumRecordAgeInSeconds == 60; GetItem(ConsistentRead=True) on each twin must return v=99.
