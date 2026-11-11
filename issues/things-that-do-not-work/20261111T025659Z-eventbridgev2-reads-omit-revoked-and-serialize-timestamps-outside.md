Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable eventbridgev2-event-bus-retention-description-and-key-validation eventbridgev2-update-event-source-configuration-rejections
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|DescribeEventSource|2xx->2xx|$.Revoked
Issue-Related: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md
Issue-Related: 20261111T025659Z-eventbridgev2-event-bus-and-event-source-updates-are.md
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridgeV2 reads omit Revoked and serialize timestamps outside the recorded AWS shape

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventSource

## Description
THREE net-new causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, rpcv2Cbor) 200-response projections: a read never reports that a resource has been revoked, and every timestamp a read emits fails the recording's timestamp shape assertion. ALL THREE must be closed and re-replayed.

Cause 1 of 3 - DescribeEventSource never reports Revoked after a successful RevokeResource (fixture eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable, step task#5).
AWS: Smithy: DescribeEventSourceResponse.Revoked targets Boolean, and RevokeResource (@idempotent, input Arn @required targeting RevocableResourceArn, which admits subscriber and event-sourcev2 ARNs) is the operation that sets it. Recorded behavior: after RevokeResource on an event source, the read of that source answers 200 with Revoked == true - the source survives the revocation (it stays readable and deletable) and the member is how a client observes that the bus owner has withdrawn it.
Emulator: the member is absent from the 200 body ($.Revoked: missing, expected True, got None). Root cause: RevokeResource is a 501 scaffold stub (services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/revoke_resource.rs:4 "SCAFFOLDED, NOT IMPLEMENTED"), so the stored source's `revoked` flag is never set, and src/ops/describe_event_source.rs:210 projects `revoked: source.revoked.then_some(true)` - i.e. it drops the member whenever the flag is false.
Why: a client cannot tell a revoked source from a live one. Revocation is the documented precondition for deleting the parent bus and for refusing updates to the source, so every branch keyed on Revoked ("is it safe to delete the bus?", "why was my update refused?") reads "not revoked" against the emulator and the opposite against AWS.
Fix: the RevokeResource handler (set the stored flag) together with the DescribeEventSource response projection that reports it. The auto-filed scaffold-stub issue for RevokeResource covers the 501 answer only; this read divergence is the state/projection consequence and must be re-replayed against this step.
Traces: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#5. Code: src/ops/revoke_resource.rs (stub handler), src/ops/describe_event_source.rs:207-210.

Cause 2 of 3 - DescribeEventBus emits CreationTime, LastModifiedTime and StorageConfiguration.RetentionWindowStartTime in a value shape the recording does not use (fixture eventbridgev2-event-bus-retention-description-and-key-validation, step task#10).
AWS: Smithy: DescribeEventBusResponse.CreationTime and .LastModifiedTime and StorageConfigurationOutput.RetentionWindowStartTime all target com.amazonaws.eventbridgev2#Timestamp (type timestamp); the service's single protocol is rpcv2Cbor, whose timestamps are epoch-second values carried as a tagged CBOR date-time. Recorded behavior only for the exact value: the harness shape-asserts these members (expected '<shaped-not-asserted>'), i.e. the recorded values match the timestamp shape of the recording and their per-run epoch numbers are NOT the requirement.
Emulator: all three values decode as bare fractional numbers carrying sub-millisecond precision and FAIL that assertion. Hypothesis: src/state.rs:505-513 `to_date_time` builds the DateTime from secs plus the full `subsec_nanos()`, so the encoder emits a fractional number at nanosecond precision rather than the tagged, millisecond-precision epoch timestamp AWS reports.
Why: an SDK that decodes the member as a number instead of an instant loses the typed value, so date comparisons, waiters and any time arithmetic on a bus's timestamps behave differently against the emulator than against AWS.
Fix: the timestamp serialization behind these read projections - src/state.rs:505 `to_date_time` and the rpcv2Cbor timestamp encoding it feeds.
Traces: eventbridgev2-event-bus-retention-description-and-key-validation#task#10.

Cause 3 of 3 - DescribeEventSource emits CreationTime and LastModifiedTime in the same non-conforming shape (fixture eventbridgev2-update-event-source-configuration-rejections, step task#12).
AWS: Smithy: DescribeEventSourceResponse.CreationTime and .LastModifiedTime target the same Timestamp shape, carried as rpcv2Cbor timestamps; the recording shape-asserts both.
Emulator: both come back as bare fractional epoch numbers with sub-millisecond precision and fail the assertion (src/ops/describe_event_source.rs:211-212 call the same `crate::state::to_date_time`).
Why: as Cause 2, on the event-source read path - this is the only 200-body divergence left on that step, so a client that is otherwise correct still cannot treat the member as an instant.
Fix: the same timestamp serialization as Cause 2, re-replayed against the event-source read because it is a separate projection and a separate failing step.
Traces: eventbridgev2-update-event-source-configuration-rejections#task#12.

Related open issue that does NOT cover these three: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md reports the same timestamp-shape defect from the CreateEventBus/DescribeEventBus steps of fixture eventbridgev2-subscriber-delivers-published-event-to-sqs and says nothing about Revoked; it carries none of the replay keys above, so a fix validated only against it leaves these three steps unrun.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; 200 steps, 63 passed, 109 mismatched, 28 blocked). All three clusters are service=eventbridgev2, verdict mismatch, expected_status 200 / actual_status 200.

Cause 1 - eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#5, DescribeEventSource:
  $.Revoked: missing (expected True, got None)
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794364853.666267)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794364853.666267)

Cause 2 - eventbridgev2-event-bus-retention-description-and-key-validation#task#10, DescribeEventBus:
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794365250.19573)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794365250.69573)
  $.StorageConfiguration.RetentionWindowStartTime: value_mismatch (expected '<shaped-not-asserted>', got 1794365250.19573)

Cause 3 - eventbridgev2-update-event-source-configuration-rejections#task#12, DescribeEventSource:
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794365273.969182)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794365273.969182)

The epoch numbers above are this run's clock and are NOT the requirement - only the value's shape/precision is.

Emulator source: src/ops/revoke_resource.rs:4 "SCAFFOLDED, NOT IMPLEMENTED" (the revoke never records anything); src/ops/describe_event_source.rs:210 `revoked: source.revoked.then_some(true)` (member dropped when the flag is false) and 211-212 `crate::state::to_date_time(...)`; src/state.rs:505-513 `to_date_time` = `DateTime::from_secs_and_nanos(secs, since_epoch.subsec_nanos())`.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): DescribeEventSourceResponse {EventSourceArn, Name, EventBusArn, Configuration, Description, State, Revoked (Boolean), CreationTime (Timestamp), LastModifiedTime (Timestamp)}; DescribeEventBusResponse {..., StorageConfiguration -> StorageConfigurationOutput{RetentionPeriodInDays @range 1..365, RetentionWindowStartTime Timestamp "The earliest point in time from which stored events are available"}, CreationTime, LastModifiedTime}; RevokeResource {Arn @required -> RevocableResourceArn}; service AWSEventsV2 v2025-05-15, protocol rpcv2Cbor.

## Steps to Reproduce
1. git_clone the repo, boot the emulator built from it and replay trace set simparity-wire-aws-v1.

Cause 1 (eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#5): CreateEventBus (wait ACTIVE), CreateEventSource on it, RevokeResource(Arn=<source arn>) (the emulator answers 501), then DescribeEventSource on that ARN. AWS answers 200 with Revoked == true; the emulator's 200 body has no Revoked member at all.

Cause 2 (eventbridgev2-event-bus-retention-description-and-key-validation#task#10): create a bus with a StorageConfiguration and DescribeEventBus it. Decode CreationTime, LastModifiedTime and StorageConfiguration.RetentionWindowStartTime from the rpcv2Cbor body: the emulator emits bare fractional epoch numbers with sub-millisecond precision instead of the recorded timestamp shape.

Cause 3 (eventbridgev2-update-event-source-configuration-rejections#task#12): DescribeEventSource on a source created earlier in the fixture and decode CreationTime/LastModifiedTime - same non-conforming shape.
