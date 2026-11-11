Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-update-event-bus-description-and-retention
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-update-event-bus-description-and-retention eventbridgev2-update-event-source-configuration-and-description
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.StorageConfiguration.RetentionPeriodInDays
Issue-Key: eventbridgev2|DescribeEventBus|2xx->2xx|$.StorageConfiguration.RetentionWindowStartTime
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.CreationTime
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.Description
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.LastModifiedTime
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.Configuration.AwsServiceEventsConfiguration.AwsService
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.Configuration.AwsServiceEventsConfiguration.Pattern
Issue-Related: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridgeV2 event-bus and event-source updates are never applied, so reads report pre-update state

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventBus

## Description
THREE net-new causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, rpcv2Cbor). An accepted update to an event bus or an event source leaves the stored resource untouched, so every subsequent read reports the resource exactly as it was created. The read handlers themselves are right; the mutation never happens, because UpdateEventBus and UpdateEventSource are still 501 scaffold stubs. ALL THREE causes must be closed and re-replayed.

Cause 1 of 3 - DescribeEventBus reports the pre-update Description and RetentionPeriodInDays (fixture eventbridgev2-update-event-bus-description-and-retention, step task#4; the same cluster also covers task#6 and task#8, the two later reads in the same flow).
AWS: Smithy: UpdateEventBus takes Description (@length 0..512) and StorageConfiguration (RetentionPeriodInDays @range 1..365) and its own documentation states "Updates an event bus. The update is asynchronous: the bus moves to UPDATING and returns to ACTIVE when the change is applied. Fields omitted from the request are left unchanged."; DescribeEventBusResponse carries Description and StorageConfiguration -> StorageConfigurationOutput{RetentionPeriodInDays, RetentionWindowStartTime}. Recorded behavior: after UpdateEventBus changes the description and the retention period, the bus settles back to ACTIVE and every read of it reports the NEW values - the recorded reads show Description 'updated description' and RetentionPeriodInDays 7 against a bus created with 'initial description' and 3.
Emulator: all three reads still answer 200 with the creation-time values ($.Description: expected 'updated description', got 'initial description'; $.StorageConfiguration.RetentionPeriodInDays: expected 7, got 3). The UpdateEventBus call in the flow is answered 501 NotImplemented (services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/update_event_bus.rs:196 returns Scaffolded::not_implemented), so no write ever reaches the stored bus.
Why: a client that updates a bus and then reads it back - to confirm the change, to drive a waiter on State/Description, or to recompute a retention window - sees its update silently discarded, and the emulator's account state diverges from AWS's from that point on for every later step of the flow.
Fix: the UpdateEventBus handler (its `handle` body and the bus state it must mutate) - apply Description/EncryptionConfiguration/StorageConfiguration to the stored bus, move it through UPDATING back to ACTIVE, and leave omitted fields unchanged. The auto-filed scaffold-stub issue for UpdateEventBus covers the 501 ANSWER only; this read divergence is the state consequence and must be re-replayed against this step to confirm the fix.
Traces: eventbridgev2-update-event-bus-description-and-retention#task#4, #task#6, #task#8. Code: src/ops/update_event_bus.rs:179-197, src/state.rs (the stored bus record).

Cause 2 of 3 - DescribeEventSource reports the pre-update Description (fixture eventbridgev2-update-event-source-configuration-and-description, step task#5).
AWS: Smithy: UpdateEventSourceRequest.Description (@length 0..512) and DescribeEventSourceResponse.Description; event sources are provisioned synchronously, so the read immediately after a successful update reports the new value. Recorded behavior: the read shows Description 'second description' after an update from 'first description'.
Emulator: the read answers 200 with Description 'first description' ($.Description: value_mismatch). UpdateEventSource answers 501 (src/ops/update_event_source.rs is a SCAFFOLDED stub), so nothing is written.
Why: as Cause 1 - a confirmed-write-then-read pattern is unobservable, and anything keyed on the updated description never fires.
Fix: the UpdateEventSource handler and the stored event-source record it must mutate.
Traces: eventbridgev2-update-event-source-configuration-and-description#task#5.

Cause 3 of 3 - DescribeEventSource reports the pre-update Configuration union, including a Pattern the update removed (fixture eventbridgev2-update-event-source-configuration-and-description, step task#7).
AWS: Smithy: UpdateEventSourceRequest.Configuration targets the EventSourceConfiguration union ("Exactly one variant must be set"), whose AwsServiceEventsSourceConfiguration carries AwsService (@required, @pattern ^aws\.[a-z0-9\-]+$) and an optional Pattern; DescribeEventSourceResponse.Configuration reports it. Recorded behavior: an update that supplies AwsServiceEventsConfiguration{AwsService: 'aws.s3'} with no Pattern REPLACES the stored variant wholesale - the recorded read reports AwsService 'aws.s3' and NO Pattern member at all, i.e. the configuration is replaced, not merged into the previous one.
Emulator: the read still reports AwsService 'aws.ec2' and the old Pattern '{"detail-type": ["EC2 Instance State-change Notification"]}' is still present, along with the stale Description of Cause 2.
Why: a client that re-points a source at a different AWS service, or clears a filter pattern by omitting it, sees neither change; worse, against the emulator the OLD filter keeps selecting events, so delivery behavior differs from AWS even where the client never reads the configuration back.
Fix: the UpdateEventSource handler - replace the stored Configuration union with the one supplied (and only then project it on reads), rather than leaving the created value in place.
Traces: eventbridgev2-update-event-source-configuration-and-description#task#7.

The CreationTime/LastModifiedTime shape mismatches that appear in the same three clusters are a SEPARATE cause (the timestamps are serialized outside AWS's shape) and are filed in the companion bundle on event-source/event-bus read projections from this same replay; they are not part of this issue. Related open issues that do NOT cover these three: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md (create/describe projection of a NEW bus, no update in the flow) and 20261110T210822Z-eventbridgev2-3-net-new-behavioral-causes-deleteeventbus-is.md (DeleteEventBus gating, StorageConfiguration omission, bus not-found message); neither carries any of the replay keys above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; 200 steps, 63 passed, 109 mismatched, 28 blocked). All three clusters are service=eventbridgev2, verdict mismatch, expected_status 200 / actual_status 200.

Cause 1 - eventbridgev2-update-event-bus-description-and-retention#task#4, DescribeEventBus (affected_count 3: #task#4, #task#6, #task#8):
  $.Description: value_mismatch (expected 'updated description', got 'initial description')
  $.StorageConfiguration.RetentionPeriodInDays: value_mismatch (expected 7, got 3)
  $.CreationTime / $.LastModifiedTime / $.StorageConfiguration.RetentionWindowStartTime: value_mismatch (expected '<shaped-not-asserted>') - the separate timestamp cause, filed elsewhere.

Cause 2 - eventbridgev2-update-event-source-configuration-and-description#task#5, DescribeEventSource:
  $.Description: value_mismatch (expected 'second description', got 'first description')

Cause 3 - eventbridgev2-update-event-source-configuration-and-description#task#7, DescribeEventSource:
  $.Configuration.AwsServiceEventsConfiguration.AwsService: value_mismatch (expected 'aws.s3', got 'aws.ec2')
  $.Configuration.AwsServiceEventsConfiguration.Pattern: unexpected_present (expected None, got '{"detail-type": ["EC2 Instance State-change Notification"]}')
  $.Description: value_mismatch (expected 'second description', got 'first description')

Emulator source: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/update_event_bus.rs:4 carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and line 196 returns Scaffolded::not_implemented("UpdateEventBus"); src/ops/update_event_source.rs:4 carries the same marker. The read paths are implemented (src/ops/describe_event_bus.rs, src/ops/describe_event_source.rs:200-213) and faithfully report the un-mutated records.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): UpdateEventBus input {EventBusArn @required, Description, EncryptionConfiguration, StorageConfiguration}, output State (BusState incl. UPDATING) + LastModifiedTime, doc "Fields omitted from the request are left unchanged."; UpdateEventSource input {EventSourceArn @required, Configuration (EventSourceConfiguration union), Description}; DescribeEventSourceResponse members EventSourceArn, Name, EventBusArn, Configuration, Description, State, Revoked, CreationTime, LastModifiedTime.

## Steps to Reproduce
1. git_clone the repo, boot the emulator built from it and replay trace set simparity-wire-aws-v1.

Cause 1 (eventbridgev2-update-event-bus-description-and-retention#task#4/#6/#8): CreateEventBus with Description 'initial description' and StorageConfiguration.RetentionPeriodInDays 3; wait for ACTIVE. Call UpdateEventBus(EventBusArn=<arn>, Description='updated description', StorageConfiguration={RetentionPeriodInDays: 7}) - the emulator answers 501 NotImplemented. Then DescribeEventBus on the same ARN: it answers 200 still carrying 'initial description' and RetentionPeriodInDays 3, where AWS reports the updated values on this and the two later reads in the flow.

Cause 2 (eventbridgev2-update-event-source-configuration-and-description#task#5): CreateEventSource on an ACTIVE bus with Description 'first description', then UpdateEventSource(EventSourceArn=<arn>, Description='second description') (501 from the emulator), then DescribeEventSource: Description is still 'first description'.

Cause 3 (eventbridgev2-update-event-source-configuration-and-description#task#7): on the same source, UpdateEventSource(Configuration={AwsServiceEventsConfiguration:{AwsService:'aws.s3'}}) with no Pattern, then DescribeEventSource: the emulator still reports AwsService 'aws.ec2' and the original Pattern, where AWS reports 'aws.s3' with no Pattern member.
