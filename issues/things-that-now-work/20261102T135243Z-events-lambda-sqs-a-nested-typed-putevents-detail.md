Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb
Issue-Key: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb|run#38|Timeout|resp = ddb.get_item(TableName=table, Key={"pk": {"S": marker}}, ConsistentRead=True)
Issue-Commit: 6a9be084645386b6802b5798f23439463b3480cb

# events__lambda__sqs: a nested typed PutEvents detail never survives the EventBridge→SQS→Lambda-ESM chain — neither hop delivers, so the queue message and the DynamoDB write never appear (1 cause)

> _Note: this issue was identified against the code base at commit `6a9be084645386b6802b5798f23439463b3480cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause. NOTE: 20261029T182228Z-events-lambda-sqs-eventbridge-sqs-targets-ignore-the.md lists this check id in its Issue-Trace-Ids, but its stored replay key for it (`run#38|Timeout|time.sleep(1)`) no longer binds — the check now fails with the harness-level verdict `Check timed out after 60.0s`, so the replay report did not mark this failure as OWNED and it is reported here as a surviving, unowned failure. A fixer closing that issue must re-replay this check explicitly; it is not covered by the key that issue carries.

Cause 1 of 1: a nested, typed `detail` payload published with PutEvents to a custom bus never reaches the rule's SQS target, and in turn never reaches the Lambda event-source mapping that consumes the queue, so the round-trip of the payload across both hops cannot happen at all.

AWS: Smithy (com.amazonaws.eventbridge) PutEventsRequestEntry.Detail is documented as "A valid JSON object. There is no other schema imposed. The JSON object may contain fields and nested sub-objects." PutEvents answers 200 with FailedEntryCount 0 and an EventId per accepted entry. A rule on that bus whose EventPattern matches `source` dispatches the event to each configured target; for an SQS target whose queue policy Allows sqs:SendMessage to the service principal events.amazonaws.com with aws:SourceArn equal to the rule ARN, EventBridge enqueues ONE message whose body is the EventBridge envelope. In that envelope `source` and `detail-type` are exactly the values submitted, and `detail` is the submitted JSON object byte-for-structure unchanged: nested sub-objects, arrays, JSON numbers with a fractional part, booleans and JSON null are all preserved as the same JSON types (no stringification, no coercion of null, no reordering-induced loss). An enabled Lambda event-source mapping on the same queue then invokes the function with the message in Records[].body, and the function's own writes (here a DynamoDB PutItem of the parsed detail) land, after which the queue drains to ApproximateNumberOfMessages=0 and ApproximateNumberOfMessagesNotVisible=0.

Emulator: nothing is delivered on either hop. The check publishes the entry (PutEvents is accepted), then polls the rule's SQS target for up to 120s re-emitting on a dropped first attempt, and no message ever arrives; the check is cut off by the harness with `Check timed out after 60.0s` before it can reach the second hop (the Lambda ESM → DynamoDB assertion) at all. So the emulator produces: zero messages on the EventBridge SQS target, consequently zero function invocations from the event-source mapping, and no DynamoDB item for the event's marker key. The queue URL, bus name, rule name, table name and the `marker` value in the recording are per-run generated values; the requirement is exactly one envelope message per matched rule, with `detail` structurally identical to the submitted object, and exactly one DynamoDB item written by the consumer.

Why: a client that publishes a typed, nested domain payload onto a custom bus and fans it out through SQS into a Lambda consumer gets silence — PutEvents reports success (FailedEntryCount 0) while nothing is ever delivered. Because the pipeline never produces a single message, the representation guarantee the chain exists to provide (that a nested detail with sub-objects, arrays, fractional numbers, booleans and nulls survives both hops unaltered) is not merely wrong, it is unobservable, so any JSON-fidelity regression in the envelope serializer is invisible too.

Fix: the EventBridge rule dispatcher's SQS-target invoker (envelope construction and enqueue on the matched custom bus) and the Lambda event-source-mapping poller for SQS that must then invoke the function; the envelope serializer must carry PutEventsRequestEntry.Detail through as a JSON object rather than a re-encoded or coerced value.

Traces: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb

## Evidence
Failing check: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb — verdict `mismatch`, detail `Error: Check timed out after 60.0s`.

Expected (AWS): after `put_events(Entries=[{EventBusName: <bus>, Source: <source>, DetailType: "sim2real.nested-detail.test", Detail: json.dumps({marker, total: 42.75, expedited: true, giftNote: null, customer: {name, tier}, items: [{sku, qty}, {sku, qty}]})}])` returns FailedEntryCount 0, `receive_message` on the rule's SQS target returns one message whose JSON body satisfies `body["detail"] == <the submitted object>` and `body["source"] == <source>` and `body["detail-type"] == "sim2real.nested-detail.test"`; then, with an enabled event-source mapping (BatchSize 1) on that queue, `get_item` on the table for `pk = <marker>` returns an item whose `detail_json` parses back to the same object; then both ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible read "0".

Actual (emulator): PutEvents is accepted, but the target queue yields no message across the check's bounded re-emit loop, so the check never gets past `assert raw_body["detail"] == nested_detail` / `assert item is not None, f"no DynamoDB write for marker {marker!r} within 120s"` and is terminated by the harness at 60s. No status/error-code divergence is involved: the refusal is silent (HTTP 200 on PutEvents, empty ReceiveMessage), which is why the only observable is the timeout.

## Steps to Reproduce
1. CreateTable with a single String hash key `pk` (PAY_PER_REQUEST) and wait for ACTIVE.
2. CreateQueue (VisibilityTimeout 30) and read its QueueArn.
3. CreateEventBus with a generated custom bus name.
4. PutRule on that bus with EventPattern {"source": ["<source>"]} and capture the RuleArn.
5. SetQueueAttributes on the queue with a Policy allowing Principal {"Service": "events.amazonaws.com"} Action sqs:SendMessage on the queue ARN, Condition ArnEquals aws:SourceArn = the RuleArn.
6. PutTargets on the rule with the queue ARN as target Id "q".
7. PutEvents one entry on that bus with Source=<source>, DetailType="sim2real.nested-detail.test" and Detail = a nested JSON object containing a sub-object, an array of objects, a fractional number (42.75), a boolean (true) and a JSON null. Assert FailedEntryCount == 0.
8. ReceiveMessage on the queue (WaitTimeSeconds 2, VisibilityTimeout 0), re-emitting the entry as needed for up to 120s. OBSERVED: no message ever arrives. EXPECTED: exactly one message whose body's `detail` equals the submitted object and whose `source`/`detail-type` equal the submitted values.
9. Create an execution role allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the queue and dynamodb:PutItem on the table, create a Python function that writes each record's parsed `detail` to the table keyed by `detail.marker`, and CreateEventSourceMapping(EventSourceArn=<queue arn>, BatchSize=1, Enabled=True); wait for State Enabled.
10. GetItem (ConsistentRead) for pk=<marker>. OBSERVED: never written (unreachable — step 8 already produced nothing). EXPECTED: an item whose `detail_json` parses back to the submitted object, after which the queue's ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible both read "0".
