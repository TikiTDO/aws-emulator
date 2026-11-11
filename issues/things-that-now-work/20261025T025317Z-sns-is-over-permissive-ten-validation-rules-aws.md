Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.add-permission-invalid-action-rejected sns.add-permission-lifecycle-and-duplicate-label sns.create-topic-invalid-attribute-creates-nothing sns.create-topic-with-different-tags-rejected sns.topic-name-ending-in-fifo-without-the-fifo-topic-attribute-rejected sns.display-name-over-100-characters-rejected sns.set-topic-attributes-cross-account-feedback-role-rejected sns.set-topic-attributes-malformed-feedback-role-arn-rejected sns.topic-delivery-policy-computes-effective-policy sns.topic-maximum-message-size-above-256kib-requires-eligible-subscriptions sns.topic-mutable-attributes-round-trip-and-clear sns.empty-vs-absent-message-attribute-rejected sns.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected sns.message-attribute-name-length-and-period-rules-rejected sns.publish-non-bmp-message-attribute-value-rejected sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected sns.publish-with-a-reserved-attribute-name-prefix-rejected sns.publish-size-limit-counts-message-attributes sns.publish-size-counts-attribute-name-type-and-value-but-not-the-subject sns.subscribe-fifo-queue-to-standard-topic-rejected sns.fifo-topic-rejects-email-protocol sns.firehose-subscribe-without-a-subscription-role-rejected sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it sns.filter-policy-unknown-operator-rejected sns.filter-policy-over-150-value-combinations-rejected sns.filter-policy-message-body-scope-counts-nested-leaf-keys-toward-the-key-limit sns.filter-policy-scope-switch-away-from-message-body-rejected-while-policy-is-nested sns.filter-policy-operator-vocabulary-round-trip sns.subscription-filter-policy-update-and-validation sns.subscribe-same-endpoint-is-idempotent sns.tag-resource-reserved-aws-prefix-rejected sns.topic-arn-of-another-region-rejected-by-a-regional-endpoint sns.same-topic-name-in-two-regions-is-two-independent-topics
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.add-permission-invalid-action-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.add-permission-lifecycle-and-duplicate-label|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sns.create-topic-invalid-attribute-creates-nothing|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.create-topic-with-different-tags-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.topic-name-ending-in-fifo-without-the-fifo-topic-attribute-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.display-name-over-100-characters-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.set-topic-attributes-cross-account-feedback-role-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: sns.set-topic-attributes-malformed-feedback-role-arn-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: sns.topic-delivery-policy-computes-effective-policy|_err#25|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Key: sns.topic-maximum-message-size-above-256kib-requires-eligible-subscriptions|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.topic-mutable-attributes-round-trip-and-clear|run#17|AssertionError|assert "KmsMasterKeyId" not in final, (
Issue-Key: sns.empty-vs-absent-message-attribute-rejected|_error#14|AssertionError|raise AssertionError(f"the call was expected to be rejected but SUCCEEDED, returning {result!r}")
Issue-Key: sns.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.message-attribute-name-length-and-period-rules-rejected|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.publish-non-bmp-message-attribute-value-rejected|_error_code#8|AssertionError|raise AssertionError(
Issue-Key: sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.publish-with-a-reserved-attribute-name-prefix-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.publish-size-limit-counts-message-attributes|_err#11|AssertionError|raise AssertionError(
Issue-Key: sns.publish-size-counts-attribute-name-type-and-value-but-not-the-subject|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.subscribe-fifo-queue-to-standard-topic-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.fifo-topic-rejects-email-protocol|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: sns.firehose-subscribe-without-a-subscription-role-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.filter-policy-unknown-operator-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: sns.filter-policy-over-150-value-combinations-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.filter-policy-message-body-scope-counts-nested-leaf-keys-toward-the-key-limit|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.filter-policy-scope-switch-away-from-message-body-rejected-while-policy-is-nested|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.filter-policy-operator-vocabulary-round-trip|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: sns.subscription-filter-policy-update-and-validation|run#7|AssertionError|raise AssertionError(f"SetSubscriptionAttributes with a malformed FilterPolicy SUCCEEDED: {result!r}")
Issue-Key: sns.subscribe-same-endpoint-is-idempotent|_error#11|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.tag-resource-reserved-aws-prefix-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sns.topic-arn-of-another-region-rejected-by-a-regional-endpoint|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.same-topic-name-in-two-regions-is-two-independent-topics|run#20|AssertionError|assert (_listed(sns, a), _listed(sns, b)) == (True, False), (
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS is over-permissive: ten validation rules AWS enforces are not enforced, so invalid control-plane calls answer 200 (10 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Each cause below is a request real AWS refuses with a modeled 400 InvalidParameterException (awsQueryError code InvalidParameter) that the emulator accepts with HTTP 200, leaving invalid state behind.

Cause 1 of 10: AddPermission accepts an ActionName that is not an SNS action, and accepts a Label that already exists.
AWS: Smithy AddPermission errors include InvalidParameterException (400, code InvalidParameter); recorded messages "Invalid parameter: Policy statement action out of service scope!" and "Invalid parameter: Statement already exists". The rejected call must not insert or modify a policy statement.
Emulator: both calls return 200 (empty AddPermissionResponse) and the statement is written.
Why: a topic ends up with a policy statement naming a nonexistent action, or a silently overwritten statement, so authorization tests built on labels are meaningless.
Fix: the AddPermission handler's validation.
Traces: sns.add-permission-invalid-action-rejected, sns.add-permission-lifecycle-and-duplicate-label.

Cause 2 of 10: CreateTopic accepts an unknown/invalid Attributes entry, a reused name with different Tags, and a name ending in ".fifo" without FifoTopic=true.
AWS: all three are refused with InvalidParameter and nothing is created (a reused name is idempotent only when attributes and tags match).
Emulator: 200 with a TopicArn (e.g. ...:sim2real-behav-sns-fifosfx-c8a154788e37.fifo created with no FifoTopic attribute).
Why: clients get a topic whose configuration they never asked for, and a .fifo-named standard topic that cannot behave like either kind.
Fix: the CreateTopic handler's attribute/tag/name validation.
Traces: sns.create-topic-invalid-attribute-creates-nothing, sns.create-topic-with-different-tags-rejected, sns.topic-name-ending-in-fifo-without-the-fifo-topic-attribute-rejected.

Cause 3 of 10: SetTopicAttributes accepts out-of-range and malformed values, and will not clear KmsMasterKeyId.
AWS: refuses DisplayName longer than 100 characters, a feedback-role ARN in another account, a malformed feedback-role ARN, an invalid DeliveryPolicy document, and MaximumMessageSize above 256 KiB when the topic has subscriptions not eligible for it — all InvalidParameter 400. Setting KmsMasterKeyId to the empty string removes the attribute.
Emulator: 200 for each refusal case; an emptied KmsMasterKeyId reads back as '' instead of being absent.
Why: invalid topic configuration is accepted and later behaves unpredictably; a client cannot turn encryption off.
Fix: the SetTopicAttributes handler's per-attribute validation and attribute-clearing path.
Traces: sns.display-name-over-100-characters-rejected, sns.set-topic-attributes-cross-account-feedback-role-rejected, sns.set-topic-attributes-malformed-feedback-role-arn-rejected, sns.topic-delivery-policy-computes-effective-policy, sns.topic-maximum-message-size-above-256kib-requires-eligible-subscriptions, sns.topic-mutable-attributes-round-trip-and-clear.

Cause 4 of 10: Publish accepts invalid MessageAttributes.
AWS: refuses with InvalidParameter an attribute with an empty value, a value placed in the field that does not match its DataType (e.g. BinaryValue for String), a name longer than 256 characters or containing a leading/trailing/consecutive period, a value containing non-BMP code points, a Number outside the supported precision/scale, and a name using a reserved AWS. / Amazon. prefix.
Emulator: 200 with a MessageId for all six.
Why: malformed attributes are published and filter policies/consumers see values AWS would never have accepted.
Fix: the MessageAttributes validation shared by the Publish and PublishBatch handlers.
Traces: sns.empty-vs-absent-message-attribute-rejected, sns.message-attribute-value-in-the-wrong-field-for-its-data-type-rejected, sns.message-attribute-name-length-and-period-rules-rejected, sns.publish-non-bmp-message-attribute-value-rejected, sns.number-attribute-beyond-the-precision-or-scale-bounds-rejected, sns.publish-with-a-reserved-attribute-name-prefix-rejected.

Cause 5 of 10: the 256 KiB publish size limit is computed from the body only.
AWS: Smithy Publish$Message doc — "a message can be at most 256 KiB in size (262,144 bytes...)"; the size counts each attribute's name, data type and value, and does NOT count Subject. A body plus attributes over the limit is refused with InvalidParameter.
Emulator: a publish whose body+attributes exceed 262,144 bytes is accepted with 200, and a body sized against the Subject is treated differently from AWS.
Why: a client that would be throttled/refused by AWS succeeds here, then fails in production.
Fix: the size accounting in the Publish/PublishBatch validation.
Traces: sns.publish-size-limit-counts-message-attributes, sns.publish-size-counts-attribute-name-type-and-value-but-not-the-subject.

Cause 6 of 10: Subscribe accepts protocol/queue-type combinations AWS refuses.
AWS: refuses (InvalidParameter 400) subscribing a FIFO queue to a standard topic, protocol `email` on a FIFO topic, protocol `firehose` without SubscriptionRoleArn, FifoThroughputScope on a standard topic, and a subscription RedrivePolicy whose dead-letter target is not a FIFO queue when the subscription is on a FIFO topic.
Emulator: 200, returning a SubscriptionArn (or "pending confirmation") in each case.
Why: subscriptions exist that can never deliver, and the topic's subscription counts/listings describe an impossible topology.
Fix: the Subscribe handler's protocol/endpoint compatibility validation.
Traces: sns.subscribe-fifo-queue-to-standard-topic-rejected, sns.fifo-topic-rejects-email-protocol, sns.firehose-subscribe-without-a-subscription-role-rejected, sns.fifo-throughput-scope-on-a-standard-topic-or-an-unknown-value-rejected, sns.fifo-subscription-redrive-requires-a-fifo-dlq-and-dead-letters-into-it.

Cause 7 of 10: filter policies are stored without being validated.
AWS: Subscribe/SetSubscriptionAttributes refuse an unknown operator, more than 150 value combinations (FilterPolicyLimitExceededException/InvalidParameter), more nested leaf keys than the key limit under MessageBody scope, a scope switch away from MessageBody while the stored policy is nested, and a malformed FilterPolicy document.
Emulator: 200 for each; the invalid policy is stored.
Why: a policy that AWS would reject is accepted, so filtering behaviour diverges silently for the life of the subscription.
Fix: the filter-policy validator used by the Subscribe and SetSubscriptionAttributes handlers.
Traces: sns.filter-policy-unknown-operator-rejected, sns.filter-policy-over-150-value-combinations-rejected, sns.filter-policy-message-body-scope-counts-nested-leaf-keys-toward-the-key-limit, sns.filter-policy-scope-switch-away-from-message-body-rejected-while-policy-is-nested, sns.filter-policy-operator-vocabulary-round-trip, sns.subscription-filter-policy-update-and-validation.

Cause 8 of 10: a repeated Subscribe for the same topic/protocol/endpoint with different attributes is accepted instead of refused.
AWS: refuses the conflicting re-subscribe (recorded behavior: real AWS rejects the call); an identical re-subscribe is idempotent and returns the existing ARN.
Emulator: 200 with a NEW SubscriptionArn, creating a duplicate subscription.
Why: the endpoint ends up subscribed twice and receives duplicate notifications, and the client cannot rely on Subscribe being idempotent.
Fix: the Subscribe handler's existing-subscription lookup.
Traces: sns.subscribe-same-endpoint-is-idempotent.

Cause 9 of 10: TagResource accepts reserved aws:-prefixed tag keys.
AWS: Smithy TagResource models InvalidParameterException (400) / TagPolicyException; a key beginning with the reserved aws: prefix is refused.
Emulator: 200, and the reserved tag is stored on the topic.
Why: user tags collide with the reserved namespace used by stack/system tags, and tag-based tooling mis-attributes resources.
Fix: the TagResource handler's key validation.
Traces: sns.tag-resource-reserved-aws-prefix-rejected.

Cause 10 of 10: regional isolation is not enforced.
AWS: a regional endpoint refuses a topic ARN naming another region (InvalidParameter), and two topics of the same name in two regions are independent — each region lists only its own.
Emulator: GetTopicAttributes for an out-of-region ARN answers 200 with the local topic's attributes, and the region's listing is not scoped to that region.
Why: a client's region mistake silently reads/writes the wrong region's resource, and multi-region tests cannot distinguish the two topics.
Fix: the ARN-region check in the SNS request walk / topic lookup in state.
Traces: sns.topic-arn-of-another-region-rejected-by-a-regional-endpoint, sns.same-topic-name-in-two-regions-is-two-independent-topics.

## Evidence
Representative replay output (all with HTTPStatusCode 200 where AWS answers 400 InvalidParameter):
- sns.add-permission-invalid-action-rejected: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
- sns.create-topic-invalid-attribute-creates-nothing: ... SUCCEEDED and returned {'TopicArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-sns-badcreate-...'}
- sns.publish-size-limit-counts-message-attributes: expected the oversized publish to be rejected, but it SUCCEEDED and returned {'MessageId': ...}
- sns.subscribe-fifo-queue-to-standard-topic-rejected: expected the call to be refused, but it succeeded: {'SubscriptionArn': 'arn:aws:sns:...:sim2real-behav-sns-t-...:1fade893-...'}
- sns.filter-policy-over-150-value-combinations-rejected: expected the call to be refused, but it succeeded: {... 'HTTPStatusCode': 200 ...}
- sns.topic-arn-of-another-region-rejected-by-a-regional-endpoint: expected the call to be refused, but it succeeded: {'Attributes': {...local topic...}}
- sns.topic-mutable-attributes-round-trip-and-clear: an empty KmsMasterKeyId must remove the attribute, got ''

## Steps to Reproduce
Per cause, against the emulator's SNS endpoint:
1. CreateTopic(Name="t"); AddPermission(TopicArn, Label="l", AWSAccountId=[acct], ActionName=["NotARealSnsAction"]) -> 200 (AWS: 400 InvalidParameter); repeat AddPermission with the same Label -> 200.
2. CreateTopic(Name="t", Attributes={"NotAnAttribute":"x"}) -> 200; CreateTopic(Name="x.fifo") with no FifoTopic -> 200.
3. SetTopicAttributes(DisplayName=<101 chars>) -> 200; SetTopicAttributes(KmsMasterKeyId="") then GetTopicAttributes -> ''.
4. Publish with MessageAttributes={"a":{"DataType":"String","StringValue":""}} -> 200; with a 256 KiB body plus attributes -> 200.
5. Subscribe(Protocol="email", Endpoint=..., TopicArn=<fifo topic>) -> 200; Subscribe(Protocol="firehose") with no SubscriptionRoleArn -> 200.
6. Subscribe with Attributes={"FilterPolicy": '{"a":[{"bogus-op":1}]}'} -> 200.
7. Subscribe the same endpoint twice with different attributes -> two distinct SubscriptionArns.
8. TagResource(Tags=[{"Key":"aws:foo","Value":"v"}]) -> 200.
9. GetTopicAttributes with a topic ARN whose region is not the endpoint's -> 200.
