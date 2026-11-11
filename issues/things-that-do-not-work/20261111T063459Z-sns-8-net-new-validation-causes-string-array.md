Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.custom-attribute-type-labels-and-string-array-survive-both-delivery-modes sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected sns.filter-policy-unknown-operator-rejected sns.firehose-subscribe-without-a-subscription-role-rejected sns.message-attribute-name-length-and-period-rules-rejected sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected sns.publish-batch-validation-is-request-level
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|Publish|ParameterValueInvalid
Issue-Key: sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it|run#9|AssertionError|assert (code, message) == (
Issue-Key: sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.filter-policy-unknown-operator-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: sns.firehose-subscribe-without-a-subscription-role-rejected|run#12|AssertionError|assert message == (
Issue-Key: sns.message-attribute-name-length-and-period-rules-rejected|run#16|AssertionError|assert not wrong, f"attribute-name refusals differ from the rules: {wrong!r}"
Issue-Key: sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.publish-batch-validation-is-request-level|run#18|AssertionError|assert message == expected_message, (
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# sns: 8 net-new validation causes — String.Array publishes refused, Number precision/scale and FifoThroughputScope and filter-policy operators not validated at all, and five refusal messages are not AWS's

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Eight distinct net-new root causes in SNS's request validation. Each is numbered with its own failing check ids; ALL EIGHT must be closed before this issue is done.

Cause 1 of 8: Publish refuses a String.Array message attribute whose value is not a JSON array, on a publish AWS accepts.
AWS: recorded behavior only — Smithy models MessageAttributeValue.DataType as a plain String and puts no constraint on what a "String.Array" value contains. The recording shows Publish accepting MessageAttributes {"loose": {"DataType": "String.Array", "StringValue": "not-a-json-array"}} with HTTP 200 and a MessageId, and the value arriving verbatim both in the notification envelope (as {"Type": "String.Array", "Value": "<the published value>"}) and as an SQS message attribute on a raw-delivery subscription, alongside custom labels Number.float, String.json and Binary.gif.
Emulator: Publish answers 400 ParameterValueInvalid (InvalidParameterValueException) with "The message attribute '<name>' with type 'String.Array' must carry a JSON array of strings, numbers, true, false and null." — an error class and a message AWS does not emit here, and the whole publish is lost.
Why: a publisher using SNS's custom type labels has its message rejected outright instead of delivered, and the error it gets is not one AWS can produce for that request.
Fix: Publish's message-attribute validation in the sns Publish handler — String.Array values are opaque and must not be parsed as a precondition.
Traces: sns.custom-attribute-type-labels-and-string-array-survive-both-delivery-modes.

Cause 2 of 8: the FIFO-subscription RedrivePolicy refusal message is re-worded.
AWS: SetSubscriptionAttributes refuses a RedrivePolicy whose deadLetterTargetArn is a standard queue on a FIFO subscription of a FIFO topic. Smithy: SetSubscriptionAttributes models InvalidParameterException with awsQueryError code "InvalidParameter" and httpError 400. The message is the fixed text "Invalid parameter: RedrivePolicy: must use a FIFO queue as DLQ for a FIFO Subscription to a FIFO Topic." (no values from the request), and the refused policy is not stored.
Emulator: the code and status are right, but the message is "Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a FIFO SQS queue".
Why: all RedrivePolicy refusals share one error code, so the message is the only signal of which rule failed; a client matching on AWS's text sees none.
Fix: the SetSubscriptionAttributes RedrivePolicy validation message in the sns subscription-attribute validator.
Traces: sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it.

Cause 3 of 8: FifoThroughputScope is not validated at all — every illegal write is accepted.
AWS: SetTopicAttributes refuses FifoThroughputScope in three cases, all InvalidParameter / HTTP 400 (Smithy: SetTopicAttributes -> InvalidParameterException, awsQueryError "InvalidParameter", httpError 400), and no refusal changes the stored value: on a STANDARD topic the message is the fixed text "Invalid parameter: AttributeName"; an unrecognised value on a FIFO topic gives "Invalid parameter: Invalid FifoThroughputScope: <the submitted value>" (the value comes from the request, the rest is fixed); and narrowing back from MessageGroup to Topic gives the fixed text "Invalid parameter: Modifying FifoThroughputScope from MessageGroup to Topic is not supported". A FIFO topic does accept FifoThroughputScope=MessageGroup and reads it back.
Emulator: SetTopicAttributes(AttributeName="FifoThroughputScope", AttributeValue="MessageGroup") on a STANDARD topic answers HTTP 200 with no error at all.
Why: a client believes it configured per-message-group throughput on a topic that cannot have it, and the attribute space of a standard topic silently grows a FIFO-only key.
Fix: the SetTopicAttributes attribute validation in the sns topic-attribute handler (allowed attribute names per topic type, the value enum, and the no-downgrade rule).
Traces: sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected.

Cause 4 of 8: filter-policy operators are not validated — an unimplemented match type is accepted and stored.
AWS: SetSubscriptionAttributes refuses a FilterPolicy that uses a match type or operator SNS does not implement with InvalidParameter / HTTP 400 and leaves the previously stored policy untouched. The message is 'Invalid parameter: FilterPolicy: <reason>\n at [Source: (String)"<the submitted document>"; line: <line>, column: <column>]' — the wrapper and the per-class reason are fixed text, while the submitted document, line and column are values from the request. The reason classes recorded are "Unrecognized match type <name>" (e.g. greater-than, and the key under an unsupported $and), "Unrecognized numeric range operator: <op>" (e.g. !=), "Unsupported anything-but pattern: <name>" (e.g. cidr), and "exists match pattern must be either true or false.". Implemented operators (numeric ranges, prefix/suffix, anything-but variants, $or, equals-ignore-case) round-trip byte-for-byte.
Emulator: SetSubscriptionAttributes with FilterPolicy {"price":[{"greater-than":5}]} answers HTTP 200 and stores the document, so no refusal is produced for any of the five unsupported-operator classes.
Why: a typo'd or unsupported operator is accepted at configuration time and then silently never matches, so the subscriber loses messages with no error anywhere.
Fix: the FilterPolicy parser/validator used by Subscribe and SetSubscriptionAttributes in the sns subscription-attribute path.
Traces: sns.filter-policy-unknown-operator-rejected.

Cause 5 of 8: the firehose Subscribe-without-a-role refusal message is re-worded.
AWS: Subscribe with Protocol="firehose" and no SubscriptionRoleArn attribute is refused with InvalidParameter / HTTP 400 (Smithy: Subscribe -> InvalidParameterException, awsQueryError "InvalidParameter", httpError 400) and the fixed message "Invalid parameter: Delivery protocol firehose can only be subscribed with subscription role arn" (no values from the request), leaving no firehose subscription on the topic.
Emulator: the code is right, the message is "Invalid parameter: Attributes Reason: SubscriptionRoleArn is required for the firehose protocol".
Why: the message is the only thing distinguishing this refusal from every other InvalidParameter on Subscribe.
Fix: the Subscribe protocol/attribute validation message in the sns Subscribe handler.
Traces: sns.firehose-subscribe-without-a-subscription-role-rejected.

Cause 6 of 8: all four message-attribute-NAME length/period refusal messages are re-worded.
AWS: Publish refuses an illegal message-attribute name with ParameterValueInvalid / HTTP 400 (Smithy: Publish -> InvalidParameterValueException, awsQueryError "ParameterValueInvalid", httpError 400) and delivers nothing. The messages are fixed text, one per rule: a name longer than 256 bytes -> "Length of message attribute name must be less than 256 bytes."; a name starting with '.' -> "Invalid message attribute name starting with character '.' was found."; a name ending with '.' -> "Invalid message attribute name ending with character '.' was found."; a name with successive periods -> "Message attribute name can not have successive '.' character.". A 256-byte name and "a.b.c" are accepted and delivered.
Emulator: the code is right for all four, every message is different: "Message attribute name must be shorter than 256 bytes.", "Message attribute name must not begin with character '.'.", "Message attribute name must not end with character '.'.", "Message attribute name must not have successive '.' characters.".
Why: all four refusals share one error code, so a client can only tell the rules apart by message; none of these four strings is one AWS emits.
Fix: the message-attribute-name validation messages in the sns Publish validation path.
Traces: sns.message-attribute-name-length-and-period-rules-rejected.

Cause 7 of 8: Number message-attribute precision and scale bounds are not enforced at all.
AWS: Publish refuses a Number attribute outside its numeric bounds with ParameterValueInvalid / HTTP 400 (Smithy: Publish -> InvalidParameterValueException, awsQueryError "ParameterValueInvalid") and delivers nothing. More than 38 significant digits gives "Message attribute '<name>' number value [<value>] must have less than 38 precision digits."; a magnitude outside 10^-128..10^126 gives "Message attribute '<name>' number value [<value>] scale must be 10^-128 and 10^126." — the attribute name and the submitted value are values from the request, everything else including the two bounds is fixed text. A 38-digit value and in-scale values (e.g. at 10^125 and 10^-128) are accepted and must arrive verbatim, unrounded.
Emulator: Publish with a 39-digit Number attribute answers HTTP 200 with a MessageId; the out-of-scale values are likewise accepted, so no bound is enforced and the oversized value is delivered.
Why: a value AWS would reject is accepted and fanned out, so a consumer that relies on SNS's numeric bounds receives attributes it cannot represent, and the publisher gets no signal.
Fix: Number message-attribute value validation (precision and scale) in the sns Publish validation path.
Traces: sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected.

Cause 8 of 8: PublishBatch's malformed-Number refusal message is not AWS's.
AWS: a PublishBatch whose entry carries a Number attribute that is not a number is refused at the REQUEST level with ParameterValueInvalid / HTTP 400 and the message "Could not cast message attribute '<name>' value to number." — the attribute name is a value from the request, the rest is fixed text; no sibling entry is delivered.
Emulator: the request-level scope and the error code are now correct (the earlier per-entry scoping was closed by 20261025T025317Z-sns-fifo-deduplication-does-not-deduplicate-publishbatch-validates.md), but the message is "Message attribute '<name>' number value [<value>] is not a number.".
Why: a client matching the batch-level cast failure on AWS's wording never recognises it.
Fix: the PublishBatch message-attribute validation message in the sns PublishBatch handler (it must use the batch cast wording, not the single-Publish numeric wording).
Traces: sns.publish-batch-validation-is-request-level.

## Evidence
Cause 1: botocore InvalidParameterValueException: An error occurred (ParameterValueInvalid) when calling the Publish operation: The message attribute 'loose' with type 'String.Array' must carry a JSON array of strings, numbers, true, false and null. — expected HTTP 200 with a MessageId and the value delivered verbatim in both envelope and raw modes.

Cause 2: AssertionError: a standard DLQ on a FIFO subscription must be refused, got 'InvalidParameter' 'Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a FIFO SQS queue' — expected ('InvalidParameter', 'Invalid parameter: RedrivePolicy: must use a FIFO queue as DLQ for a FIFO Subscription to a FIFO Topic.').

Cause 3: AssertionError: expected the call to be refused, but it succeeded: HTTPStatusCode 200 — SetTopicAttributes(FifoThroughputScope=MessageGroup) on a standard topic; expected ('InvalidParameter', 'Invalid parameter: AttributeName').

Cause 4: AssertionError: expected the call to be refused, but it succeeded and returned HTTPStatusCode 200 — SetSubscriptionAttributes FilterPolicy {"price":[{"greater-than":5}]}; expected InvalidParameter with 'Invalid parameter: FilterPolicy: Unrecognized match type greater-than\n at [Source: (String)"{"price":[{"greater-than":5}]}"; line: 1, column: 28]'.

Cause 5: AssertionError: the refusal must name the missing SubscriptionRoleArn; got 'Invalid parameter: Attributes Reason: SubscriptionRoleArn is required for the firehose protocol' — expected 'Invalid parameter: Delivery protocol firehose can only be subscribed with subscription role arn'.

Cause 6: AssertionError: attribute-name refusals differ from the rules: {'n'*20: ('ParameterValueInvalid', 'Message attribute name must be shorter than 256 bytes.'), '.lead': ('ParameterValueInvalid', "Message attribute name must not begin with character '.'."), 'trail.': ('ParameterValueInvalid', "Message attribute name must not end with character '.'."), 'a..b': ('ParameterValueInvalid', "Message attribute name must not have successive '.' characters.")} — expected, in order: 'Length of message attribute name must be less than 256 bytes.', "Invalid message attribute name starting with character '.' was found.", "Invalid message attribute name ending with character '.' was found.", "Message attribute name can not have successive '.' character.".

Cause 7: AssertionError: expected the call to be refused, but it succeeded: {'MessageId': '<id>', ... 'HTTPStatusCode': 200} — Publish with MessageAttributes {'n': {'DataType': 'Number', 'StringValue': '123456789012345678901234567890123456789'}}; expected ('ParameterValueInvalid', "Message attribute 'n' number value [123456789012345678901234567890123456789] must have less than 38 precision digits."), and for '1e127'/'1E+126'/'1e-129' the scale message.

Cause 8: AssertionError: expected the exact message "Could not cast message attribute 'broken' value to number.", got "Message attribute 'broken' number value [not-a-number] is not a number."

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateTopic, subscribe one envelope and one RawMessageDelivery=true SQS queue, then Publish with MessageAttributes carrying DataType 'String.Array' and StringValue 'not-a-json-array' (plus Number.float, String.json, Binary.gif). Replay sns.custom-attribute-type-labels-and-string-array-survive-both-delivery-modes.
3. Cause 2: on a FIFO topic's FIFO subscription, SetSubscriptionAttributes RedrivePolicy with a STANDARD queue as deadLetterTargetArn. Replay sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it.
4. Cause 3: SetTopicAttributes(AttributeName='FifoThroughputScope', AttributeValue='MessageGroup') on a standard topic; then on a FIFO topic set an unknown value, and set 'Topic' after 'MessageGroup'. Replay sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected.
5. Cause 4: SetSubscriptionAttributes FilterPolicy with {"price":[{"greater-than":5}]}, {"$and":[...]}, {"price":[{"numeric":["!=",5]}]}, {"ip":[{"anything-but":{"cidr":"10.0.0.0/8"}}]} and {"coupon":[{"exists":"yes"}]}. Replay sns.filter-policy-unknown-operator-rejected.
6. Cause 5: Subscribe with Protocol='firehose' and no SubscriptionRoleArn attribute. Replay sns.firehose-subscribe-without-a-subscription-role-rejected.
7. Cause 6: Publish with a 257-character attribute name, '.lead', 'trail.' and 'a..b'. Replay sns.message-attribute-name-length-and-period-rules-rejected.
8. Cause 7: Publish with a Number attribute of 39 digits, and with '1e127', '1E+126', '1e-129'. Replay sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected.
9. Cause 8: PublishBatch with three entries where the middle one has {'DataType': 'Number', 'StringValue': 'not-a-number'}. Replay sns.publish-batch-validation-is-request-level.
