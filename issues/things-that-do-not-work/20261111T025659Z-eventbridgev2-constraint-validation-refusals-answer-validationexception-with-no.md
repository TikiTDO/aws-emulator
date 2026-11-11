Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-event-bus-retention-description-and-key-validation
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-event-bus-retention-description-and-key-validation eventbridgev2-put-events-reserved-source-and-detail-validation eventbridgev2-resource-policy-public-access-and-reserved-names eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable eventbridgev2-update-event-source-configuration-rejections
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|UpdateEventBus|4xx->4xx|$.Message
Issue-Key: eventbridgev2|UpdateEventBus|4xx->4xx|$.__type
Issue-Key: eventbridgev2|PutEvents|4xx->4xx|$.Message
Issue-Key: eventbridgev2|PutEvents|4xx->4xx|$.__type
Issue-Key: eventbridgev2|PutResourcePolicy|4xx->4xx|$.Message
Issue-Key: eventbridgev2|PutResourcePolicy|4xx->4xx|$.__type
Issue-Key: eventbridgev2|RevokeResource|4xx->4xx|$.Message
Issue-Key: eventbridgev2|RevokeResource|4xx->4xx|$.__type
Issue-Key: eventbridgev2|UpdateEventSource|4xx->4xx|$.Message
Issue-Key: eventbridgev2|UpdateEventSource|4xx->4xx|$.__type
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridgeV2 constraint-validation refusals answer ValidationException with no Message instead of the modeled InvalidInputException

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventBus

## Description
FIVE net-new causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, protocol rpcv2Cbor, apiVersion 2025-05-15). On five different operations the emulator refuses an invalid request with the right HTTP status but the wrong error identity: the body carries `__type` `ValidationException` - a shape none of these operations model - and no `Message` member at all, where AWS answers the operation's modeled InvalidInputException with its wire discriminator `com.amazon.nexus.v20250515.exceptions#InvalidInputException` and AWS's constraint-violation message. ALL FIVE causes must be closed and re-replayed before this issue is resolved.

The shared mechanism is the service's pre-handler constraint-validation layer and the error channel it uses (`crate::scaffolded::Scaffolded::validation`, which mints a 400 `ValidationException` with no message), but each cause is a separate operation/member and a separate failing step.

Cause 1 of 5 - UpdateEventBus retention-range refusal (fixture eventbridgev2-event-bus-retention-description-and-key-validation, step task#6; the same cluster also covers task#7).
AWS: Smithy: UpdateEventBusRequest.StorageConfiguration targets com.amazonaws.eventbridgev2#StorageConfiguration whose RetentionPeriodInDays has @range min 1 max 365; UpdateEventBus models InvalidInputException (@error client, @httpError 400, single member `Message`, doc "A request parameter is missing or not valid.") and does NOT model ValidationException. The recording answers HTTP 400 with `__type` `com.amazon.nexus.v20250515.exceptions#InvalidInputException` and `Message` following AWS's standard range template: fixed text "<n> validation error(s) detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have value less than or equal to <max>"; the variable parts are the violation count, the value exactly as supplied, the lowerCamel member path (`storageConfiguration.retentionPeriodInDays`) and the member's modeled maximum. The recorded instance is "1 validation error detected: Value '366' at 'storageConfiguration.retentionPeriodInDays' failed to satisfy constraint: Member must have value less than or equal to 365" - the literal 366 is this run's input filling the template.
Emulator: HTTP 400 (status correct) with `__type` `ValidationException` and `Message` absent entirely.
Why: an SDK matches the discriminator against the operation's modeled errors, so `ValidationException` does not deserialize into InvalidInputException - the client sees an unknown service error instead of the typed exception it catches, every `except InvalidInputException` branch is dead code, and with no `Message` there is nothing to log about which member failed.
Fix: the EventBridgeV2 error serializer plus the pre-handler constraint-validation layer - emit the operation's modeled InvalidInputException with its wire discriminator and populate `Message` from the range template above.
Traces: eventbridgev2-event-bus-retention-description-and-key-validation#task#6 (and #task#7). Code: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/scaffolded.rs (the `validation` constructor) and the shared constraint-validation walk that calls it.

Cause 2 of 5 - PutEvents required-member refusal inside the entry list (fixture eventbridgev2-put-events-reserved-source-and-detail-validation, step task#4).
AWS: Smithy: PutEventsRequestEntry.Source and .DetailType are both @required; PutEvents models InvalidInputException (400). The recording answers 400 with `__type` `com.amazon.nexus.v20250515.exceptions#InvalidInputException` and `Message` using the multi-violation form of the same template: fixed text "<n> validation errors detected: " then one "Value null at '<memberPath>' failed to satisfy constraint: Member must not be null" clause per violation, joined with "; "; the variable parts are the count, pluralized ("errors" above one), and the member paths, which name list elements as `entries.<1-based index>.member.<member>`. The recorded instance is "2 validation errors detected: Value null at 'entries.1.member.source' failed to satisfy constraint: Member must not be null; Value null at 'entries.2.member.detailType' failed to satisfy constraint: Member must not be null".
Emulator: HTTP 400 with `__type` `ValidationException` and no `Message`.
Why: as Cause 1 - the typed error never deserializes, and a client cannot tell WHICH entry of a batch was malformed because the per-entry member path is the only thing that says so.
Fix: same component as Cause 1; the member path must be rendered for list/structure members, not just top-level ones.
Traces: eventbridgev2-put-events-reserved-source-and-detail-validation#task#4.

Cause 3 of 5 - PutResourcePolicy PolicyName pattern refusal (fixture eventbridgev2-resource-policy-public-access-and-reserved-names, step task#8).
AWS: Smithy: PutResourcePolicyRequest.PolicyName targets PolicyName with @length 1..128 and @pattern ^[A-Za-z0-9_]+$; PutResourcePolicy models InvalidInputException (400). The recording answers 400 with the modeled discriminator and `Message` "1 validation error detected: Value 'bad-name' at 'policyName' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_]+" - fixed text "validation error detected: Value", "at", "failed to satisfy constraint: Member must satisfy regular expression pattern:"; variable parts: the count, the rejected value as supplied, the member name `policyName`, and the member's modeled pattern.
Emulator: HTTP 400 with `__type` `ValidationException` and no `Message`.
Why: as Cause 1.
Fix: same component as Cause 1. NOTE: the PutResourcePolicy handler itself is still a 501 scaffold stub (open issue 20261104T183027Z-eventbridgev2-putresourcepolicy-stub.md), but this refusal is produced BEFORE the handler runs, so the wire type and message are wrong independently of that stub and implementing the handler alone does not fix this step.
Traces: eventbridgev2-resource-policy-public-access-and-reserved-names#task#8.

Cause 4 of 5 - RevokeResource Arn pattern refusal (fixture eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable, step task#11).
AWS: Smithy: RevokeResourceRequest.Arn is @required, targets RevocableResourceArn with @length 1..1011 and a pattern admitting only subscriber and event-sourcev2 ARNs; RevokeResource models InvalidInputException (400). Passing an event-busv2 ARN violates it, and the recording answers 400 with the modeled discriminator and the pattern template of Cause 3, member name `arn`: "1 validation error detected: Value '<arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>".
Emulator: HTTP 400 with `__type` `ValidationException` and no `Message`.
Why: as Cause 1.
Fix: same component as Cause 1. Observation for the fixer: the pattern the recorded message quotes admits a third alternative, `webhook`, inside the event-sourcev2 group, which the on-disk RevocableResourceArn @pattern does not list; the message's variable part is the member's pattern as the service holds it. Hypothesis: on-disk model drift against the recorded service - reproduce the pattern the recording shows for this message.
Traces: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#11.

Cause 5 of 5 - UpdateEventSource Configuration union refusal (fixture eventbridgev2-update-event-source-configuration-rejections, step task#8; the same cluster also covers task#10).
AWS: Smithy: UpdateEventSourceRequest.Configuration targets com.amazonaws.eventbridgev2#EventSourceConfiguration, a union documented "Discriminated EventSource configuration. Exactly one variant must be set." (variants AwsServiceEventsConfiguration, PartnerEventsConfiguration); UpdateEventSource models InvalidInputException (400). A request setting neither or both variants is refused 400 with the modeled discriminator and `Message` exactly "Exactly one Configuration sub-field must be provided" - entirely FIXED text, nothing interpolated from the request or state.
Emulator: HTTP 400 with `__type` `ValidationException` and no `Message`.
Why: as Cause 1, and here the message is the only thing telling the caller the union arity rule was the problem.
Fix: same component as Cause 1, with this error class's constant message.
Traces: eventbridgev2-update-event-source-configuration-rejections#task#8 (and #task#10).

Related open issue that does NOT cover these five: 20261104T173733Z-eventbridgev2-error-responses-omit-modeled-exception-type-and.md reports the same wire defect for CreateEventBus's name-pattern refusal and the DescribeEventBus not-found message only, and carries none of the replay keys above (different operations, different members, different fixture steps), so it cannot close them.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, /__admin/health 200; 200 steps, 63 passed, 109 mismatched, 28 blocked). All five clusters are service=eventbridgev2, verdict mismatch, expected_status 400 / actual_status 400.

Cause 1 - eventbridgev2-event-bus-retention-description-and-key-validation#task#6, UpdateEventBus (cluster also covers #task#7):
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException')
  $.Message: missing (expected "1 validation error detected: Value '366' at 'storageConfiguration.retentionPeriodInDays' failed to satisfy constraint: Member must have value less than or equal to 365", got None)

Cause 2 - eventbridgev2-put-events-reserved-source-and-detail-validation#task#4, PutEvents:
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException')
  $.Message: missing (expected "2 validation errors detected: Value null at 'entries.1.member.source' failed to satisfy constraint: Member must not be null; Value null at 'entries.2.member.detailType' failed to satisfy constraint: Member must not be null", got None)

Cause 3 - eventbridgev2-resource-policy-public-access-and-reserved-names#task#8, PutResourcePolicy:
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException')
  $.Message: missing (expected "1 validation error detected: Value 'bad-name' at 'policyName' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_]+", got None)

Cause 4 - eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#11, RevokeResource:
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException')
  $.Message: missing (expected "1 validation error detected: Value 'arn:aws:events:us-west-2:000000000000:event-busv2/<bus>/<id>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z0-9]+)*:events:[a-z][a-z0-9]*(-[a-z0-9]+)*:([0-9]{12}):(subscriber|event-sourcev2\\/(aws\\.service|aws\\.partner|webhook))\\/[A-Za-z0-9][\\.\\-_A-Za-z0-9]{0,255}\\/[a-z0-9]{25}", got None) - the ARN in the recorded value is this run's input.

Cause 5 - eventbridgev2-update-event-source-configuration-rejections#task#8, UpdateEventSource (cluster also covers #task#10):
  $.__type: value_mismatch (expected 'com.amazon.nexus.v20250515.exceptions#InvalidInputException', got 'ValidationException')
  $.Message: missing (expected 'Exactly one Configuration sub-field must be provided', got None)

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): InvalidInputException @error client @httpError 400, single `Message` member, doc "A request parameter is missing or not valid."; it is declared on UpdateEventBus, PutEvents, PutResourcePolicy, RevokeResource and UpdateEventSource, none of which declare ValidationException. StorageConfiguration.RetentionPeriodInDays @range 1..365; PutEventsRequestEntry.Source/.DetailType @required; PolicyName @pattern ^[A-Za-z0-9_]+$ @length 1..128; RevocableResourceArn @pattern as quoted above (model omits the `webhook` alternative); EventSourceConfiguration union doc "Discriminated EventSource configuration. Exactly one variant must be set."

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it, then replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the five eventbridgev2 clusters named below.

Cause 1 (eventbridgev2-event-bus-retention-description-and-key-validation#task#6): create a bus, then send an rpcv2Cbor UpdateEventBus to the eventsv2 endpoint with StorageConfiguration.RetentionPeriodInDays = 366 (above the modeled max 365). Emulator answers 400 with __type 'ValidationException' and no Message; AWS answers 400 __type com.amazon.nexus.v20250515.exceptions#InvalidInputException with the range-template Message.

Cause 2 (eventbridgev2-put-events-reserved-source-and-detail-validation#task#4): PutEvents with three entries where entry 2 omits Source and entry 3 omits DetailType. Emulator: 400 'ValidationException', no Message; AWS: 400 InvalidInputException with the two-violation "Member must not be null" message naming entries.1.member.source and entries.2.member.detailType.

Cause 3 (eventbridgev2-resource-policy-public-access-and-reserved-names#task#8): PutResourcePolicy with PolicyName 'bad-name' (violates ^[A-Za-z0-9_]+$). Emulator: 400 'ValidationException', no Message; AWS: 400 InvalidInputException with the pattern-template Message for 'policyName'.

Cause 4 (eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#11): RevokeResource with Arn set to an event-busv2 ARN. Emulator: 400 'ValidationException', no Message; AWS: 400 InvalidInputException with the pattern-template Message for 'arn'.

Cause 5 (eventbridgev2-update-event-source-configuration-rejections#task#8): UpdateEventSource whose Configuration union does not have exactly one variant set. Emulator: 400 'ValidationException', no Message; AWS: 400 InvalidInputException with Message 'Exactly one Configuration sub-field must be provided'.
