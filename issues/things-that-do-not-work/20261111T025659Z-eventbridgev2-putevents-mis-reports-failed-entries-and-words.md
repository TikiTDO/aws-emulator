Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-put-events-reserved-source-and-detail-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-put-events-reserved-source-and-detail-validation
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|PutEvents|2xx->2xx|$.Entries[*].ErrorCode
Issue-Key: eventbridgev2|PutEvents|2xx->2xx|$.Entries[*].ErrorMessage
Issue-Key: eventbridgev2|PutEvents|2xx->2xx|$.FailedEntryCount
Issue-Related: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md
Issue-Related: 20261111T025659Z-eventbridgev2-constraint-validation-refusals-answer-validationexception-with-no.md
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridgeV2 PutEvents mis-reports failed entries and words its not-found error itself

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
TWO net-new causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, rpcv2Cbor) PutEvents path: the per-entry result projection reports the wrong error identity for a refused entry and refuses one class of bad entry not at all, and the batch's not-found refusal is worded by the emulator instead of AWS. BOTH must be closed and re-replayed.

Cause 1 of 2 - the per-entry results of a partially bad batch diverge in error code, error message, failure count and minted EventId (fixture eventbridgev2-put-events-reserved-source-and-detail-validation, step task#3).
AWS: Smithy: PutEventsResultEntry {EventId (@length 1..128, "Unique event ID assigned by the service"), SequenceNumber, SuccessCode, ErrorCode (@length 1..256, doc "Error code identifying why a publish batch entry failed"), ErrorMessage (@length 1..2048)} is documented as aligning 1:1 with the request entry at the same position, and PutEventsResponse.FailedEntryCount is the number of entries that failed to publish; PutEventsRequestEntry.Source's own doc states "The `aws.` value prefix is service-reserved and cannot be used as a value" and .Detail's states "The event payload, as a valid JSON string." Recorded behavior for a three-entry batch (entry 1 Source 'aws.*', entry 2 Detail not JSON, entry 3 well-formed): HTTP 200, FailedEntryCount 2, entry 1 carries ErrorCode 'ValidationError' with ErrorMessage 'Source must not start with the reserved prefix aws.', entry 2 carries ErrorCode 'MalformedDetail' with ErrorMessage 'Detail is not valid JSON' and no EventId, and entry 3 carries a published EventId whose form is shape-asserted (EventBridge mints event ids as hyphenated lowercase UUIDs, 8-4-4-4-12). All four of those strings are FIXED text for their entry-error class - nothing is interpolated from the request.
Emulator: HTTP 200, but FailedEntryCount 1; entry 1 carries ErrorCode 'InvalidArgument' and the emulator's own sentence "Source must not start with the service-reserved prefix 'aws.'"; entry 2 is PUBLISHED with an EventId and no ErrorCode/ErrorMessage at all, because the Detail is never parsed as JSON; and entry 3's EventId is 32 undelimited lowercase hex digits rather than a UUID (the particular id is per-run; its FORM is the requirement).
Why: a producer uses FailedEntryCount plus the per-entry ErrorCode to find, retry or quarantine the bad entries of a batch. Against the emulator the reserved-source branch reports a code no client is matching on, the malformed-payload branch never fires at all (so the count under-reports and an event AWS would never have admitted is stored and later delivered), and a client that parses or correlates the event id as a UUID rejects every id the emulator mints.
Fix: the PutEvents handler's per-entry loop and its result projection, plus the event-id minting it uses - emit this service's entry-error codes and messages, parse each entry's Detail before storing it and push a refusal (no EventId, no SequenceNumber) at that position, and render the event id in hyphenated UUID form; FailedEntryCount is already counted from the results, so it follows.
Traces: eventbridgev2-put-events-reserved-source-and-detail-validation#task#3. Code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/put_events.rs:205-217 (the reserved-source refusal, which uses the INVALID_ARGUMENT constant and the emulator's wording), :275 (Detail cloned into the stored event with no parse), :308-311 (the count), and src/state.rs:450 `new_event_id` (format!("{:032x}", ...)).

Cause 2 of 2 - the PutEvents not-found refusal is worded by the emulator and leaks the whole ARN (fixture eventbridgev2-put-events-reserved-source-and-detail-validation, step task#5).
AWS: Smithy: PutEvents models ResourceNotFoundException (com.amazonaws.eventbridgev2#ResourceNotFoundException, @error client, @httpError 404, single member `Message`, doc "The resource does not exist."); EventBusArn is @required with the event-busv2 ARN pattern. Recorded behavior: publishing to a bus that is not there answers HTTP 404 with `Message` "Event bus not found: <bus name>" - fixed text "Event bus not found: " plus ONE variable part, the bus NAME taken from the requested ARN (not the full ARN, and not the generated 25-character bus identifier).
Emulator: the status (404) and the error class are right, but `Message` is a sentence it composes from the request, of the form "event bus <full event-busv2 ARN> does not exist" - different fixed wording, lower-cased, and it interpolates the whole ARN including the generated id where AWS names only the bus. The ARN in any one replay is a per-run value; the wording and the choice of variable part are the defect.
Why: the error message is part of the contract. A client or test that matches AWS's wording (or parses the bus name out of it) fails against the emulator, and conversely code that learns to read a full ARN out of the message has nothing to read against AWS.
Fix: the PutEvents handler's bus-lookup failure path and the shared ResourceNotFoundException construction it uses - src/ops/put_events.rs:168-175, which formats `format!("event bus {} does not exist", input.event_bus_arn)`.
Traces: eventbridgev2-put-events-reserved-source-and-detail-validation#task#5.

Related open issues that do NOT cover these two: 20261104T183027Z-eventbridgev2-2-net-new-behavioral-root-causes-the.md reports the missing MalformedDetail refusal from the BEHAVIORAL suite (it carries no wire fixture key, and it says nothing about the reserved-source entry's ErrorCode/ErrorMessage wording or FailedEntryCount on this batch); 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md reports the EventId form from a different fixture's step; and 20261110T210822Z-eventbridgev2-3-net-new-behavioral-causes-deleteeventbus-is.md reports the bus not-found wording on DescribeEventBus, not on PutEvents. None of them carries the replay keys above, so none can close these steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; 200 steps, 63 passed, 109 mismatched, 28 blocked). Both clusters are service=eventbridgev2, operation PutEvents, verdict mismatch.

Cause 1 - eventbridgev2-put-events-reserved-source-and-detail-validation#task#3 (expected 200, got 200):
  $.Entries[0].ErrorCode: value_mismatch (expected 'ValidationError', got 'InvalidArgument')
  $.Entries[0].ErrorMessage: value_mismatch (expected 'Source must not start with the reserved prefix aws.', got "Source must not start with the service-reserved prefix 'aws.'")
  $.Entries[1].ErrorCode: missing (expected 'MalformedDetail', got None)
  $.Entries[1].ErrorMessage: missing (expected 'Detail is not valid JSON', got None)
  $.Entries[2].EventId: value_mismatch (expected '<shaped-not-asserted>', got '6c6032449137c2e96e73e372e2338aca')
  $.FailedEntryCount: value_mismatch (expected 2, got 1)
  (the 32-hex id is per-run; its FORM - a hyphenated UUID - is the requirement)

Cause 2 - eventbridgev2-put-events-reserved-source-and-detail-validation#task#5 (expected 404, got 404):
  $.Message: value_mismatch (expected 'Event bus not found: sim2real-probe-ebv2-bus-6e682c4b', got 'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-probe-ebv2-bus-6e682c4b/a1b2c3d4e5f6a7b8c9d0e1f2a does not exist')
  (the bus name, account, region and 25-char id are this run's values filling each side's template; the fixed text "Event bus not found: " and its single bus-name variable are the requirement)

Emulator source: src/ops/put_events.rs:205-217 pushes PutEventsResultEntry{error_code: Some(INVALID_ARGUMENT), error_message: Some(format!("Source must not start with the service-reserved prefix '{RESERVED_SOURCE_PREFIX}'"))} for a reserved Source; :270-293 stores the event with `detail: entry.detail.clone()` (:275) and no JSON parse, so no MalformedDetail branch exists; :308-311 counts failures from the results (hence 1); src/state.rs:450 `new_event_id` renders the 128 bits as `{:032x}`; :168-175 formats the not-found message from the requested ARN.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): PutEvents {EventBusArn @required, Entries @required @length 1..100, DeduplicationConfiguration} -> {FailedEntryCount (Integer, default 0), Entries}; PutEventsResultEntry doc "Result entry for a published event. The position of each entry in the response list aligns 1:1 with the corresponding request entry."; ErrorCode doc "Error code identifying why a publish batch entry failed."; PutEventsRequestEntry.Source doc "The `aws.` value prefix is service-reserved and cannot be used as a value."; EventDetail doc "The event payload of a PutEvents entry, as a valid JSON string."; PutEvents errors include ResourceNotFoundException (404, member `Message`).

## Steps to Reproduce
1. git_clone the repo, boot the emulator built from it and replay trace set simparity-wire-aws-v1.

Cause 1 (eventbridgev2-put-events-reserved-source-and-detail-validation#task#3): CreateEventBus and wait for ACTIVE, then PutEvents with three entries - entry 1 Source starting with 'aws.', entry 2 a valid Source with Detail that is not JSON, entry 3 well formed. AWS answers 200 with FailedEntryCount 2, entry 1 {ErrorCode 'ValidationError', ErrorMessage 'Source must not start with the reserved prefix aws.'}, entry 2 {ErrorCode 'MalformedDetail', ErrorMessage 'Detail is not valid JSON', no EventId} and entry 3 published with a hyphenated-UUID EventId. The emulator answers FailedEntryCount 1, entry 1 {ErrorCode 'InvalidArgument', ErrorMessage "Source must not start with the service-reserved prefix 'aws.'"}, entry 2 PUBLISHED with an EventId, and entry 3's EventId as 32 undelimited hex digits.

Cause 2 (eventbridgev2-put-events-reserved-source-and-detail-validation#task#5): PutEvents with a well-formed event-busv2 EventBusArn naming a bus that does not exist. Both answer 404 ResourceNotFoundException, but AWS's Message is 'Event bus not found: <bus name>' while the emulator returns 'event bus <full arn> does not exist'.
