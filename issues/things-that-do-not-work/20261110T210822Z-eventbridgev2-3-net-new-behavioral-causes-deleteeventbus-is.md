Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected eventbridgev2.recreated-event-bus-same-name-gets-new-identifier
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked|_refused#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected|run#4|KeyError|window = ebv2.describe_event_bus(EventBusArn=bus_arn)["StorageConfiguration"]["RetentionWindowStartTime"]
Issue-Key: eventbridgev2.recreated-event-bus-same-name-gets-new-identifier|run#16|AssertionError|assert got == ("ResourceNotFoundException", f"Event bus not found: {name}"), (
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# eventbridgev2: 3 net-new behavioral causes — DeleteEventBus is not gated on un-revoked event sources, event-bus reads omit StorageConfiguration, and the bus not-found message is emulator-authored

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteEventBus

## Description
Three net-new root causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, protocol rpcv2Cbor, endpointPrefix eventsv2). They are independent and ALL THREE must be closed before this issue is resolved.

Cause 1 of 3 — DeleteEventBus succeeds on a bus that still carries a non-revoked event source, instead of refusing with ResourceInUseException (failing check: eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked).
AWS: Smithy: DeleteEventBus declares ResourceInUseException (@error client, @httpError 409), documented "The resource is in use and cannot be deleted. For example, an event bus with subscribers or event sources cannot be deleted until they are deleted."; the operation's own documentation in the model states "A bus with subscribers or event sources cannot be deleted." Recorded behavior: while an event source created by CreateEventSource on the bus has not been revoked, DeleteEventBus is refused with code ResourceInUseException and the message "The event bus cannot be deleted because it has one or more event sources that are not revoked. Delete or revoke them and retry." — that message is entirely FIXED text, it interpolates nothing from the request or state. The refused delete must leave the bus State == ACTIVE. After RevokeResource on the source ARN, DeleteEventBus succeeds, the bus transitions away and finally reads ResourceNotFoundException, while the revoked source OUTLIVES the bus: DescribeEventSource still answers 200 with Revoked == true and EventBusArn still naming the deleted bus, and DeleteEventSource then removes it.
Emulator: DeleteEventBus answers HTTP 200 with an empty body (rpc-v2-cbor, content-length 2) and moves the bus to DELETING even though an un-revoked event source exists; no error is raised at all. The handler looks up the bus and transitions it with no in-use gate on its children.
Why: a client relying on AWS's ordering constraint writes "revoke/delete the sources, then delete the bus" and has no way to observe the guard against the emulator; worse, the emulator destroys a bus AWS would have kept, so the account state diverges from that point on (the source's parent bus is gone before the source was ever revoked) and any error-handling branch keyed on ResourceInUseException/409 is dead code.
Fix: the DeleteEventBus handler's pre-delete validation and the bus/event-source relation in the service state — refuse with the modeled ResourceInUseException while any event source attached to the bus is not revoked, and leave the bus untouched. Traces/code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/delete_event_bus.rs:160-182 (the handler goes straight from lookup to the DELETING transition).

Cause 2 of 3 — DescribeEventBus never reports StorageConfiguration, so the bus's retention window is unreadable (failing check: eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected).
AWS: Smithy: DescribeEventBusResponse.StorageConfiguration targets com.amazonaws.eventbridgev2#StorageConfigurationOutput (doc: "Event storage configuration of an event bus, as returned on reads") with RetentionPeriodInDays (@range 1..365) and RetentionWindowStartTime (timestamp, doc: "The earliest point in time from which stored events are available. Events older than this have expired from retention."). Recorded behavior: a read of any bus reports its effective storage configuration, including RetentionWindowStartTime, whether or not the create request supplied one. The check needs that value to compute a POINT_IN_TIME StartingPoint before the window.
Emulator: the member is absent from the 200 DescribeEventBus body, so the client's response dict has no "StorageConfiguration" key (replay: KeyError: 'StorageConfiguration' on describe_event_bus(...)["StorageConfiguration"]["RetentionWindowStartTime"]). The projection only emits StorageConfigurationOutput when the bus was created with an explicit StorageConfiguration.
Why: a client cannot learn a bus's retention window at all and cannot distinguish "no retention" from "not reported"; anything that dereferences StorageConfiguration.RetentionWindowStartTime or RetentionPeriodInDays (replay windows, point-in-time subscribers, waiters) breaks outright. Because this check dies on the read, the whole POINT_IN_TIME rejection surface behind it (a future StartingPoint, a StartingPoint under 5 minutes old, and a StartingPoint before the retention window, each an InvalidInputException) is also never exercised and may hide further divergence once the read is fixed.
Fix: the DescribeEventBus (and CreateEventBus) response projection and the bus state that backs it — give every bus an effective, defaulted storage configuration and always project it. Code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/describe_event_bus.rs:199 (storage_configuration_output) and src/state.rs:197-203.
Related open issue that does NOT cover this check: 20261109T155235Z-eventbridgev2-event-bus-reads-omit-storageconfiguration-and-description.md reports the same omission from the WIRE corpus (keys eventbridgev2|DescribeEventBus|2xx->2xx|$.StorageConfiguration); it carries no behavioral replay key, so this behavioral check is filed here and must be re-replayed to confirm the fix.

Cause 3 of 3 — the ResourceNotFoundException message for a bus that is gone is emulator-authored and interpolates the ARN (failing check: eventbridgev2.recreated-event-bus-same-name-gets-new-identifier).
AWS: Smithy: DescribeEventBus declares ResourceNotFoundException (@error client, @httpError 404, single member Message, doc "The resource does not exist."). Recorded behavior: describing the ARN of a bus that was deleted answers code ResourceNotFoundException with Message "Event bus not found: <name>" — fixed text "Event bus not found: " followed by ONE variable part, the bus's Name taken from the requested ARN (not the full ARN, and not the generated 25-character identifier).
Emulator: answers code ResourceNotFoundException (status 404 is right) with Message of the form "event bus <full event-busv2 ARN> does not exist" — different fixed wording, lower-cased, and it leaks the whole ARN including the generated identifier where AWS names only the bus. The ARN and identifier in any one replay are per-run values and are not the requirement; the wording and the choice of variable part are.
Why: the error message is part of the contract. A client or test that matches AWS's not-found wording (or parses the bus name out of it) fails against the emulator, and the emulator leaks a generated identifier into a message AWS keeps to the name.
Fix: the DescribeEventBus handler's not-found path and the shared ResourceNotFoundException construction it uses, for every path that reports a missing bus (DeleteEventBus formats the same sentence at delete_event_bus.rs:163). Code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/describe_event_bus.rs:186-192.
Related open issue that does NOT cover this check: 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md Cause 3 reports the same handler from the WIRE corpus, where a bus that NEVER existed records the constant Message "Resource does not exist"; the two recordings must be reconciled by the fixer (a never-created ARN vs a deleted bus), but in both cases the emulator's "event bus <arn> does not exist" is wrong.

Traces: eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked (Cause 1), eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected (Cause 2), eventbridgev2.recreated-event-bus-same-name-gets-new-identifier (Cause 3).

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout (eventbridgev2: 2/17 passed).

Cause 1 — [mismatch] eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked:
  check: create bus (ACTIVE) -> CreateEventSource{Configuration={AwsServiceEventsConfiguration={AwsService: aws.ec2}}} -> DeleteEventBus
  expected: refusal ('ResourceInUseException', 'The event bus cannot be deleted because it has one or more event sources that are not revoked. Delete or revoke them and retry.'), HTTP 409, and DescribeEventBus still State=ACTIVE
  actual: AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/cbor', 'smithy-protocol': 'rpc-v2-cbor', 'content-length': '2', ...}}}

Cause 2 — [error] eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected:
  window = ebv2.describe_event_bus(EventBusArn=bus_arn)["StorageConfiguration"]["RetentionWindowStartTime"]
  expected: 200 body carrying StorageConfiguration{RetentionPeriodInDays (1..365), RetentionWindowStartTime (timestamp)}
  actual: KeyError: 'StorageConfiguration' (member absent from the 200 response)

Cause 3 — [mismatch] eventbridgev2.recreated-event-bus-same-name-gets-new-identifier:
  after DeleteEventBus, DescribeEventBus on the old ARN
  expected: ('ResourceNotFoundException', 'Event bus not found: <bus name>')
  actual: AssertionError: the deleted bus's ARN describes as ('ResourceNotFoundException', 'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-behav-ebv2-bus-3ca2c689beaa/bawe5gem8xxzeaio4y036mpj5 does not exist')
  (the bus name, account, region and the 25-char identifier above are this run's values; the fixed text and the choice of variable part are the requirement)

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): DeleteEventBus errors include ResourceInUseException (409, "...an event bus with subscribers or event sources cannot be deleted until they are deleted"), InvalidStateException (409), ResourceNotFoundException (404); DescribeEventBusResponse members include StorageConfiguration -> StorageConfigurationOutput{RetentionPeriodInDays @range 1..365, RetentionWindowStartTime timestamp} and errors ResourceNotFoundException (404, member Message).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay behavioral suite simparity-behavioral-aws-v1.

Cause 1: CreateEventBus (ACTIVE), then CreateEventSource on that bus with Configuration.AwsServiceEventsConfiguration.AwsService = aws.ec2. Call DeleteEventBus(EventBusArn=<bus arn>) WITHOUT revoking the source. The emulator answers 200 and starts deleting; AWS refuses 409 ResourceInUseException with the fixed sentence above and leaves the bus ACTIVE. Then RevokeResource(Arn=<source arn>) and DeleteEventBus: it must succeed, the bus must become not-found, and DescribeEventSource on the source must still answer Revoked=true with EventBusArn naming the deleted bus. Replay check: eventbridgev2.delete-event-bus-blocked-by-event-source-until-revoked.

Cause 2: CreateEventBus without any StorageConfiguration, wait for ACTIVE, then DescribeEventBus on its ARN and read StorageConfiguration.RetentionWindowStartTime. The member is absent from the emulator's 200 body. Replay check: eventbridgev2.point-in-time-starting-point-outside-allowed-range-rejected.

Cause 3: CreateEventBus, DeleteEventBus, poll until the bus is gone, then DescribeEventBus on the old ARN and inspect Error.Code/Error.Message: the emulator answers 404 ResourceNotFoundException with "event bus <arn> does not exist" instead of "Event bus not found: <name>". Replay check: eventbridgev2.recreated-event-bus-same-name-gets-new-identifier.
