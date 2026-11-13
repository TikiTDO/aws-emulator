Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-subscriber-delivers-published-event-to-sqs
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$
Issue-Key: sqs|GetQueueAttributes|2xx->2xx|$.Attributes.ApproximateNumberOfMessages
Issue-Key: sqs|ReceiveMessage|2xx->2xx|$
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# sqs: events published to an EventBridgeV2 bus are never delivered to the subscriber's SQS queue

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueAttributes

## Description
The end-to-end delivery leg of the recorded scenario never happens: an event accepted by PutEvents on an EventBridgeV2 event bus with a subscriber whose target is an SQS queue never becomes a message on that queue, so both SQS observations of the queue diverge. Two causes, both on service token sqs (the service the diverging steps are reported under), both symptoms of the same missing delivery path; both must be closed before this issue is resolved.

Cause 1 of 2 — GetQueueAttributes reports ApproximateNumberOfMessages '0' forever after a successful publish (fixture: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#9).
AWS: Smithy (sqs): GetQueueAttributes returns Attributes as a string map; ApproximateNumberOfMessages is the approximate number of messages available for retrieval from the queue, serialized as a decimal string. Recorded behavior: after the bus accepts the event (the publish answered 200 with a FailedEntryCount of 0), the recording's GetQueueAttributes on the subscriber's target queue reports ApproximateNumberOfMessages '1' — the delivered event is on the queue.
Emulator: the attribute stays '1'-less: $.Attributes.ApproximateNumberOfMessages: value_mismatch (expected '1', got '0'), and it never changed — the harness polled 37 times and capped at 180s. The status is 200 in both, so the divergence is the queue depth, not the response shape.
Why: a client (or a test) that publishes to an event bus and then waits for the fan-out to land on SQS waits forever and then reads an empty queue, so no EventBridge->SQS integration can be exercised against the emulator even though every control-plane call succeeded.
Fix: the event-delivery path that a subscriber owes its target — the PutEvents handler / subscriber state in services/service-com-amazonaws-eventbridgev2-awseventsv2 must, for each RUNNING non-revoked subscriber of the bus, enqueue the event onto the target SQS queue in the sqs service's state. Note the upstream blocker: CreateSubscriber in that crate is still an unimplemented 501 scaffold (services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_subscriber.rs:227), already filed as open issue 20261104T183027Z-eventbridgev2-createsubscriber-stub.md — that issue covers only the 501 answer for the create call, NOT this delivery divergence, which stays open until a published event actually lands on the target queue.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#9.

Cause 2 of 2 — ReceiveMessage on the subscriber's target queue returns no messages (fixture: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#10).
AWS: Smithy (sqs): ReceiveMessage returns Messages, a list of Message{MessageId, ReceiptHandle, MD5OfBody, Body}. Recorded behavior: the recording's ReceiveMessage against the same queue returns exactly one message whose MessageId, ReceiptHandle, MD5OfBody and Body are all present (their values are per-run and shape-asserted only, so no literal id, handle, digest or body text from the recording is a requirement).
Emulator: the 200 response body is empty — $: empty_body (expected {'Messages': [{'Body': ..., 'MD5OfBody': ..., 'MessageId': ..., 'ReceiptHandle': ...}]}, got {}). An empty body is the correct answer for an empty queue, so the defect is that the queue is empty: the delivered event was never enqueued.
Why: the consumer side of the scenario receives nothing, so a client cannot read, parse or delete the event it published; any downstream assertion on the delivered event body is unreachable.
Fix: same component as Cause 1 — once delivery enqueues the event, ReceiveMessage in the sqs service returns it with the usual MessageId/ReceiptHandle/MD5OfBody/Body members.
Traces: eventbridgev2-subscriber-delivers-published-event-to-sqs#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture eventbridgev2-subscriber-delivers-published-event-to-sqs. The bus create, the subscriber setup steps and PutEvents all answered, then:

task#9 sqs GetQueueAttributes (expected 200, got 200):
  $: poll capped at 180s (expected {'Attributes': {'ApproximateNumberOfMessages': '1'}}, got {'Attributes': {'ApproximateNumberOfMessages': '0'}})
  $.Attributes.ApproximateNumberOfMessages: value_mismatch (expected '1', got '0')
  $.Attributes.ApproximateNumberOfMessages: poll: not in ['1'] after 37 attempt(s)

task#10 sqs ReceiveMessage (expected 200, got 200):
  $: empty_body (expected {'Messages': [{'Body': '<shaped-not-asserted>', 'MD5OfBody': '<shaped-not-asserted>', 'MessageId': '<shaped-not-asserted>', 'ReceiptHandle': '<shaped-not-asserted>'}]}, got {})

Expected: one message on the subscriber's target queue per accepted event — ApproximateNumberOfMessages '1' and a ReceiveMessage result carrying one Message with MessageId, ReceiptHandle, MD5OfBody and Body. Actual: the queue stays empty indefinitely. Supporting code: CreateSubscriber is a 501 scaffold stub (services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_subscriber.rs:227) and the eventbridgev2 state (src/state.rs) holds no subscriber registry and no delivery path, so PutEvents only appends to the bus's own event log.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture eventbridgev2-subscriber-delivers-published-event-to-sqs from trace set simparity-wire-aws-v1.
2. Create an SQS queue, create an EventBridgeV2 event bus, and create a subscriber on that bus whose target is the queue.
3. Publish one event with PutEvents on that bus (it answers 200).
4. Step task#9: poll sqs GetQueueAttributes on the queue for ApproximateNumberOfMessages — it stays '0' (the harness polled 37 times over 180s) where AWS reports '1'.
5. Step task#10: call sqs ReceiveMessage on the queue — the 200 body is empty where AWS returns one Message with MessageId, ReceiptHandle, MD5OfBody and Body.
