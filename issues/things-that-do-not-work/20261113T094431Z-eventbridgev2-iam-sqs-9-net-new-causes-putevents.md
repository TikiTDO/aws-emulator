Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2__iam__sqs.put-events-reserved-aws-source-refused-and-never-delivered eventbridgev2__iam__sqs.update-subscriber-replaces-filter-configuration-whole eventbridgev2__iam__sqs.detail-pattern-matches-put-events-envelope-not-raw-payload eventbridgev2__iam__sqs.fifo-subscriber-delivers-event-group-in-publish-order eventbridgev2__iam__sqs.jsonata-transformer-string-unquoted-and-object-serialized eventbridgev2__iam__sqs.sqs-message-attributes-evaluate-jsonata-and-keep-binary-literal eventbridgev2__iam__sqs.put-events-envelope-carries-resources-time-and-region eventbridgev2__iam__sqs.put-events-explicit-deduplication-id-suppresses-repeat eventbridgev2__iam__sqs.put-events-content-based-deduplication-suppresses-resend eventbridgev2__iam__sqs.event-bus-target-forwards-as-new-event-with-evaluated-metadata eventbridgev2__iam__sqs.horizon-subscriber-replays-retained-events-marked-replay eventbridgev2__iam__sqs.dead-letter-record-names-access-denied-when-role-cannot-reach-target eventbridgev2__iam__sqs.subscriber-on-failure-configuration-writes-dead-letter-record-to-sqs eventbridgev2__iam__sqs.sequence-numbers-increase-with-ingestion-and-match-delivery eventbridgev2__iam__sqs.update-subscriber-transformer-applies-to-later-events
Issue-Key: eventbridgev2__iam__sqs.put-events-reserved-aws-source-refused-and-never-delivered|run#15|AssertionError|assert (bad.get("ErrorCode"), bad.get("ErrorMessage")) == (
Issue-Key: eventbridgev2__iam__sqs|UpdateSubscriber|NotImplemented
Issue-Key: eventbridgev2__iam__sqs.detail-pattern-matches-put-events-envelope-not-raw-payload|_warm_up#9|TimeoutError|raise TimeoutError("the run budget ended during warm-up; not graded")
Issue-Key: eventbridgev2__iam__sqs.fifo-subscriber-delivers-event-group-in-publish-order|_warm_up#9|TimeoutError|raise TimeoutError("the run budget ended during warm-up; not graded")
Issue-Key: eventbridgev2__iam__sqs.jsonata-transformer-string-unquoted-and-object-serialized|_warm_up#11|TimeoutError|raise TimeoutError("the run budget ended during warm-up; not graded")
Issue-Key: eventbridgev2__iam__sqs.sqs-message-attributes-evaluate-jsonata-and-keep-binary-literal|_warm_up#9|TimeoutError|raise TimeoutError("the run budget ended during warm-up; not graded")
Issue-Key: eventbridgev2__iam__sqs.put-events-envelope-carries-resources-time-and-region|_collect#15|TimeoutError|raise TimeoutError(f"the run budget ended before {what}; not graded")
Issue-Key: eventbridgev2__iam__sqs.put-events-explicit-deduplication-id-suppresses-repeat|_collect#21|TimeoutError|raise TimeoutError(f"the run budget ended before {what}; not graded")
Issue-Key: eventbridgev2__iam__sqs.put-events-content-based-deduplication-suppresses-resend|_receive_matching#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: eventbridgev2__iam__sqs.event-bus-target-forwards-as-new-event-with-evaluated-metadata|run#17|TimeoutError|raise TimeoutError("the run budget ended before forwarding; not graded")
Issue-Key: eventbridgev2__iam__sqs.horizon-subscriber-replays-retained-events-marked-replay|_collect#18|TimeoutError|raise TimeoutError(f"the run budget ended before {what}; not graded")
Issue-Key: eventbridgev2__iam__sqs.dead-letter-record-names-access-denied-when-role-cannot-reach-target|run#14|TimeoutError|raise TimeoutError("the run budget ended before a dead-letter record; not graded")
Issue-Key: eventbridgev2__iam__sqs.subscriber-on-failure-configuration-writes-dead-letter-record-to-sqs|_poll_dlq_for_failure_record#23|Timeout|for msg in sqs.receive_message(
Issue-Key: eventbridgev2__iam__sqs.sequence-numbers-increase-with-ingestion-and-match-delivery|_collect#16|TimeoutError|raise TimeoutError(f"the run budget ended before {what}; not graded")
Issue-Key: eventbridgev2__iam__sqs.update-subscriber-transformer-applies-to-later-events|_collect#12|TimeoutError|raise TimeoutError(f"the run budget ended before {what}; not graded")
Issue-Commit: 0275b87193e8729c4e6e84626c64cbadccfa7c1a

# eventbridgev2__iam__sqs: 9 net-new causes — PutEvents' reserved-aws. entry refusal is mis-coded, UpdateSubscriber is a stub, and filtered / transformed / FIFO / event-bus / replay / dedup / dead-letter delivery never happens

> _Note: this issue was identified against the code base at commit `0275b87193e8729c4e6e84626c64cbadccfa7c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Nine distinct root causes in the EventBridge v2 bus + subscriber data plane. CreateSubscriber and plain SQS-target delivery now work (several checks get past their warm-up poll), so these are the next layer of divergences, not the already-filed CreateSubscriber stub. Related open issues that do NOT cover these checks: 20261109T163411Z-eventbridgev2-iam-sqs-eventbridge-v2-createsubscriber-answers-notimplemented-so-no-subscriber.md (CreateSubscriber stub) and 20261111T025659Z-eventbridgev2-putevents-mis-reports-failed-entries-and-words.md (PutEvents not-found wording). A fixer must close all nine before submitting.

=== Cause 1 of 9: PutEvents refuses a reserved `aws.` Source with the wrong per-entry ErrorCode and message ===
AWS: Smithy — PutEventsRequestEntry.Source doc: "The source of the event. The `aws.` value prefix is service-reserved and cannot be used as a value." The refusal is reported per entry inside a 200 response in PutEventsResultEntry{ErrorCode, ErrorMessage}, not as a top-level error; FailedEntryCount counts it. Recorded behaviour: the result entry for the `aws.`-prefixed entry carries ErrorCode "ValidationError" and ErrorMessage "Source must not start with the reserved prefix aws." — that whole message is fixed text with no variable part — and carries neither EventId nor SuccessCode, while the sibling well-formed entry in the same request carries SuccessCode "PUBLISHED".
Emulator: the entry is refused, but with ErrorCode "InvalidArgument" and ErrorMessage "Source must not start with the service-reserved prefix 'aws.'" (extra word "service-" and quoted prefix).
Why: a client that branches on the per-entry ErrorCode (retry vs. drop, or surfacing a validation failure) sees an unknown code and mis-classifies a permanent validation failure; a client asserting the message text does not match.
Fix: the per-entry validation of PutEvents in the eventbridgev2 PutEvents handler — the Source-prefix check's error code and message string.
Traces: eventbridgev2__iam__sqs.put-events-reserved-aws-source-refused-and-never-delivered

=== Cause 2 of 9: UpdateSubscriber is a scaffolded stub ===
AWS: Smithy — UpdateSubscriber is a modelled, idempotent operation taking SubscriberArn plus Description/State/ResumePosition/InvokeConfiguration/FilterConfiguration/BatchConfiguration/Transformer/RetryPolicy/OnFailureConfiguration/LogConfiguration and returning UpdateSubscriberResponse{SubscriberArn, Name, EventBusArn, Type, StartingPosition, PointInTimeConfiguration, State, LastModifiedTime}. Its modelled errors are ConcurrentModificationException, InvalidStateException, LimitExceededException, ResourceNotFoundException, AccessDeniedException, InternalException, InvalidInputException, ThrottlingException — NotImplemented is not among them. A FilterConfiguration supplied on update replaces the previous one whole, rather than merging.
Emulator: ClientError (NotImplemented) when calling the UpdateSubscriber operation: "operation UpdateSubscriber is scaffolded but not implemented in this emulator".
Why: a subscriber's filter, transformer, retry policy or state can never be changed after creation; any client that reconfigures a subscriber in place fails with an error code outside the modelled set, so its retry/error handling cannot classify it.
Fix: the eventbridgev2 UpdateSubscriber handler and the subscriber record in service state.
Traces: eventbridgev2__iam__sqs.update-subscriber-replaces-filter-configuration-whole

=== Cause 3 of 9: a subscriber whose target is configured with a filter, a transformer, SQS message attributes or a FIFO queue never receives any event ===
AWS: recorded behaviour only. A subscriber created on an ACTIVE bus begins consuming shortly after creation, and a PutEvents entry matching the subscriber's FilterConfiguration is delivered to the target queue within seconds. The checks establish this with a warm-up loop that republishes a marker every 5 s and waits for it on the target queue; on AWS that loop completes.
Emulator: no warm-up event ever reaches those target queues, so the loop runs until the budget is gone ("the run budget ended during warm-up; not graded"). The affected subscriber shapes are: a DATA-scope FilterConfiguration pattern on the PutEvents envelope, a Transformer, SqsParameters carrying MessageAttributes, and a .fifo target queue with SystemMetadata.EventGroupId.
Why: filtering, transformation, message attributes and FIFO ordering are all unobservable — a client that routes events by pattern or relies on ordered FIFO delivery receives nothing at all, with no error anywhere to explain it.
Fix: the eventbridgev2 subscriber delivery path — filter evaluation, transformer evaluation, SQS target parameter mapping and FIFO group handling in the bus-to-target dispatcher.
Traces: eventbridgev2__iam__sqs.detail-pattern-matches-put-events-envelope-not-raw-payload, eventbridgev2__iam__sqs.fifo-subscriber-delivers-event-group-in-publish-order, eventbridgev2__iam__sqs.jsonata-transformer-string-unquoted-and-object-serialized, eventbridgev2__iam__sqs.sqs-message-attributes-evaluate-jsonata-and-keep-binary-literal

=== Cause 4 of 9: a PutEvents entry carrying Resources and/or Time is never delivered ===
AWS: Smithy — PutEventsRequestEntry.Resources ("ARNs of resources the event concerns. Included in the event delivered to subscribers.") and PutEventsRequestEntry.Time ("The time the event occurred. Defaults to the time the service receives the event when omitted."). Recorded behaviour: the delivered envelope is {version: "0", id: <generated>, detail-type, source, account, time, region, resources, detail}, where time is the entry's Time rendered to the second as ISO-8601 Z, resources is the entry's Resources list verbatim, account and region are the bus's, and version is the string "0".
Emulator: the same subscriber that received the plain warm-up events never receives the entry that carries Resources and Time; the delivery poll ends with the budget spent, so the envelope's resources/time/version/account/region cannot be read back at all.
Why: a consumer that keys off the event envelope's resources or event time never sees the event; the enriched entry is silently dropped while plain entries flow.
Fix: the eventbridgev2 PutEvents ingestion of Resources/Time and the envelope serializer in the delivery path.
Traces: eventbridgev2__iam__sqs.put-events-envelope-carries-resources-time-and-region

=== Cause 5 of 9: PutEvents deduplication is not implemented ===
AWS: Smithy — PutEvents takes DeduplicationConfiguration, and PutEventsRequestEntry carries SystemMetadata. Recorded behaviour: with content-based deduplication a byte-identical re-publish inside the dedup window is accepted (FailedEntryCount 0) but delivered once; with an explicit deduplication id a repeat carrying the same id is likewise suppressed while a control entry with a fresh id is delivered.
Emulator: neither check reaches a verdict — the control delivery never arrives ("the run budget ended before the control delivery") and the content-based check burns its whole 60 s budget. No suppression or delivery behaviour is observable.
Why: a producer that relies on EventBridge's deduplication to make a retry safe gets either duplicates or nothing, and cannot tell which.
Fix: the eventbridgev2 PutEvents handler's DeduplicationConfiguration handling and the dedup index in service state.
Traces: eventbridgev2__iam__sqs.put-events-explicit-deduplication-id-suppresses-repeat, eventbridgev2__iam__sqs.put-events-content-based-deduplication-suppresses-resend

=== Cause 6 of 9: an event-bus target never forwards the event to the second bus ===
AWS: recorded behaviour only. A subscriber whose target is another event bus re-publishes the matched event onto that bus as a new event, with the forwarding bus's account/region and evaluated metadata, so a subscriber on the second bus receives it.
Emulator: nothing is ever forwarded — the forwarding poll ends with "the run budget ended before forwarding; not graded".
Why: bus-to-bus fan-out chains are dead; an event published to the first bus never reaches consumers of the second.
Fix: the eventbridgev2 delivery dispatcher's event-bus target type.
Traces: eventbridgev2__iam__sqs.event-bus-target-forwards-as-new-event-with-evaluated-metadata

=== Cause 7 of 9: retained events are never replayed to a subscriber created after they were published ===
AWS: recorded behaviour only. Subscriber StartingPosition / PointInTimeConfiguration let a subscriber created after publication read the bus's retained events; those replayed deliveries are marked as a replay and arrive in addition to new events.
Emulator: the events published before the subscriber existed are never delivered — the poll ends with "the run budget ended before replay of the events published before the subscriber existed; not graded".
Why: a consumer onboarded onto an existing bus silently starts from empty instead of from the retained horizon, losing every event published before it subscribed.
Fix: the eventbridgev2 subscriber starting-position/replay path and the bus's event retention store.
Traces: eventbridgev2__iam__sqs.horizon-subscriber-replays-retained-events-marked-replay

=== Cause 8 of 9: no dead-letter record is ever written to OnFailureConfiguration when delivery is refused ===
AWS: recorded behaviour only (EventBridge retry/DLQ behaviour). When a subscriber's delivery role may not write to the target, EventBridge exhausts RetryPolicy.MaxRetryAttempts and writes one JSON dead-letter record to the OnFailureConfiguration.Arn queue whose fields are subscriberArn (the subscriber's ARN), targetArn (the target's ARN), retryAttempts (the configured MaxRetryAttempts, 0 here), errorCode "ACCESS_DENIED", failedMessages[0].eventId (the EventId PutEvents returned), and errorMessage whose fixed text is "EventBridge could not access <targetArn>. Check that it exists and that the subscriber's delivery role is allowed to access it." — the only variable part is the target ARN taken from the subscriber's configuration. The target itself receives nothing.
Emulator: no record ever lands on the OnFailureConfiguration queue — the poll ends with "the run budget ended before a dead-letter record; not graded". The second check, which waits for the dead-letter record written by a subscriber's OnFailureConfiguration, exhausts its whole 60 s budget the same way.
Why: a delivery misconfiguration becomes completely silent — no delivery, no DLQ record, nothing for an operator to diagnose; a consumer of the DLQ never learns an event was dropped.
Fix: the eventbridgev2 delivery dispatcher's retry/on-failure path and the dead-letter record serializer.
Traces: eventbridgev2__iam__sqs.dead-letter-record-names-access-denied-when-role-cannot-reach-target, eventbridgev2__iam__sqs.subscriber-on-failure-configuration-writes-dead-letter-record-to-sqs

=== Cause 9 of 9: per-event sequence numbers and multi-event delivery do not line up ===
AWS: Smithy — PutEventsResultEntry carries SequenceNumber alongside EventId. Recorded behaviour: sequence numbers increase with ingestion order on an ordered bus, and every one of three successively published events is delivered, carrying the sequence number its PutEvents result reported.
Emulator: only part of the batch is ever delivered — the poll for all three events ends with "the run budget ended before delivery of all three events; not graded", so neither the delivery count nor the sequence-number correspondence can be read back.
Why: a consumer that orders or de-duplicates by sequence number cannot, and events published in a burst are lost.
Fix: the eventbridgev2 PutEvents sequence-number assignment and the ordered delivery path.
Traces: eventbridgev2__iam__sqs.sequence-numbers-increase-with-ingestion-and-match-delivery

## Evidence
Cause 1 — AssertionError: the aws. entry answered {'ErrorCode': 'InvalidArgument', 'ErrorMessage': "Source must not start with the service-reserved prefix 'aws.'"}
  Expected per-entry result: ErrorCode 'ValidationError', ErrorMessage 'Source must not start with the reserved prefix aws.' (no EventId, no SuccessCode), sibling entry SuccessCode 'PUBLISHED'.

Cause 2 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateSubscriber operation: operation UpdateSubscriber is scaffolded but not implemented in this emulator.
  Expected: 200 UpdateSubscriberResponse; modelled errors are ConcurrentModification/InvalidState/LimitExceeded/ResourceNotFound/AccessDenied/Internal/InvalidInput/Throttling only.

Cause 3 — TimeoutError: the run budget ended during warm-up; not graded (filtered subscriber, transformer subscriber, SQS message-attribute subscriber, FIFO target). No warm-up event reaches those queues at all; the same warm-up loop completes for a plain SqsParameters subscriber in the same suite, so the bus and PutEvents are live.

Cause 4 — TimeoutError: the run budget ended before the delivery; not graded. Warm-up on the same subscriber succeeded; only the entry carrying Resources=[...] and Time=<timestamp> was never delivered. Expected envelope keys: version '0', detail-type, source, account, time (ISO-8601 Z to the second), region, resources, detail, plus a non-empty string id.

Cause 5 — TimeoutError: the run budget ended before the control delivery; not graded (explicit DeduplicationId); Error: Check timed out after 60.0s (content-based deduplication).

Cause 6 — TimeoutError: the run budget ended before forwarding; not graded.

Cause 7 — TimeoutError: the run budget ended before replay of the events published before the subscriber existed; not graded.

Cause 8 — TimeoutError: the run budget ended before a dead-letter record; not graded, and Error: Check timed out after 60.0s. Expected dead-letter record: {subscriberArn: <subscriber ARN>, targetArn: <target ARN>, errorCode: 'ACCESS_DENIED', retryAttempts: 0, failedMessages[0].eventId: <EventId from PutEvents>, errorMessage: 'EventBridge could not access <targetArn>. Check that it exists and that the subscriber's delivery role is allowed to access it.'} and an empty target queue.

Cause 9 — TimeoutError: the run budget ended before delivery of all three events; not graded.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateEventBus, CreateSubscriber with InvokeConfiguration {RoleArn, TargetArn: <sqs queue arn>, SqsParameters: {}}; PutEvents with two entries, the first Source='aws.sim2real', the second a non-reserved Source. Read Entries[0].ErrorCode / ErrorMessage.
3. Cause 2: call UpdateSubscriber on that subscriber with a new FilterConfiguration.
4. Cause 3: create a subscriber with a DATA-scope FilterConfiguration (or a Transformer, or SqsParameters.MessageAttributes, or a .fifo TargetArn plus SystemMetadata.EventGroupId) and PutEvents a matching entry every 5 s; poll the target queue.
5. Cause 4: on a plain subscriber that already receives events, PutEvents one entry carrying Resources=[...] and Time=<fixed timestamp>; poll the target queue.
6. Cause 5: PutEvents with DeduplicationConfiguration (content-based, then an explicit deduplication id) and re-publish; count deliveries.
7. Cause 6: create a subscriber whose TargetArn is a second event bus; publish to the first bus and poll a subscriber on the second.
8. Cause 7: publish events, then create a subscriber positioned at the retained horizon and poll it.
9. Cause 8: create a subscriber with RetryPolicy {MaxRetryAttempts: 0}, OnFailureConfiguration {Arn: <dlq arn>} and a delivery role NOT allowed sqs:SendMessage on the target; publish a matching event and poll the DLQ and the target.
10. Cause 9: PutEvents three entries in sequence on an ordered bus and compare delivered events and their SequenceNumbers.
