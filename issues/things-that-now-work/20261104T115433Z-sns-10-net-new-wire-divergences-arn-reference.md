Issue-Source: trace-sweep-divergence
Issue-Trace: sns-refarch-s3ev-topic-lookup-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-refarch-s3ev-topic-lookup-mistakes sns-cnt-list-by-topic-reference-mistakes sns-cnt-subscribe-endpoint-mistakes sns-snx-traj-raw-delivery-refused-on-lambda sns-cnt-unsubscribe-drops-one-subscriber sns-cnt-resubscribe-does-not-inflate-count sns-cnt-topic-subscription-counters-settle sns-cnt-list-subscriptions-token-refused-then-listed
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sns|GetTopicAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|GetTopicAttributes|4xx->4xx|status
Issue-Key: sns|ListSubscriptionsByTopic|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sns|ListSubscriptionsByTopic|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|ListSubscriptionsByTopic|4xx->4xx|status
Issue-Key: sns|Subscribe|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: sns|Subscribe|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|GetSubscriptionAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|Subscribe|4xx->2xx|$.ErrorResponse
Issue-Key: sns|Subscribe|4xx->2xx|status
Issue-Key: sns|GetTopicAttributes|2xx->2xx|$.GetTopicAttributesResponse.GetTopicAttributesResult.Attributes.entry[*].value
Issue-Key: sns|ListSubscriptions|2xx->2xx|$.ListSubscriptionsResponse.ListSubscriptionsResult.NextToken
Issue-Commit: 3bd56f3581a93dde5b982de16e37f58616506edc

# sns: 10 net-new wire divergences — ARN reference mistakes answer the wrong status, code and message, and four reads/writes report the wrong state

> _Note: this issue was identified against the code base at commit `3bd56f3581a93dde5b982de16e37f58616506edc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicAttributes

## Description
Replaying the wire fixture corpus simparity-wire-aws-v1 against the booted emulator leaves 17 mismatched steps, of which 12 are net-new (the other 5 are owned by 20261030T200727Z-sns-5-wire-divergences-the-opt-out-check.md and are not re-filed here). Those 12 steps cluster into the TEN independent root causes below, all of service `sns`. Each cause is numbered and carries its own fixture and step. ALL TEN must be closed before this issue is submitted.

None of the ten was reported as blocked by an unimplemented operation, and none was reported as owned by an open issue. Neighbouring open issues that do NOT cover and do NOT carry these replay keys: 20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md (other validation rules), 20261030T200727Z-sns-5-wire-divergences-the-opt-out-check.md (the SMS/opt-out and subscription-text causes), and 20261102T093047Z-sns-sqs-4-net-new-root-causes-an.md, whose Cause 4 is the behavioral twin of Cause 8 below under the `sns__sqs` token with no wire replay key — Cause 8 is filed here so the wire step that proves it is actually re-replayed.

=== Cause 1 of 10: a TopicArn naming another ACCOUNT is answered 404 NotFound instead of AWS's 403 InvalidClientTokenId (GetTopicAttributes) ===
Fixture/step: sns-refarch-s3ev-topic-lookup-mistakes#task#3.
AWS: Smithy (com.amazonaws.sns#GetTopicAttributes) declares AuthorizationErrorException (403/AuthorizationError), InvalidParameterException (400/InvalidParameter) and NotFoundException (404/NotFound) — it declares NO error for an ARN whose account id is not an account the endpoint can resolve, so this is recorded behavior only. The recording shows the call refused with HTTP 403 and ErrorResponse.Error.Code = 'InvalidClientTokenId' and Error.Message = 'No account found for the given parameters'. That message is WHOLLY FIXED text: nothing in it is taken from the request or from state (the ARN is not echoed), and the refusal happens at the credential/account-resolution layer, BEFORE the topic store is consulted — so it does not depend on whether a topic of that name exists anywhere.
Emulator: the call is answered HTTP 404 with Error.Code = 'NotFound' and Error.Message = 'Topic does not exist'. The handler's pre-store validation only checks that the TopicArn parses and names the served region (`state::regional_arn_refusal("TopicArn", …)` in ops/get_topic_attributes.rs); the ARN's ACCOUNT ID is never compared with the account this endpoint serves, so an ARN of a foreign account falls through to the ordinary store lookup and is reported as a missing topic.
Why: a client cannot tell 'you are using the wrong credentials/account' from 'this topic was deleted'. 403/InvalidClientTokenId is a non-retryable credential fault a client surfaces to its operator; 404/NotFound is routinely treated as 'create it' or 'already gone'. So cross-account misconfiguration is silently reinterpreted as a missing resource, and provisioning code happily re-creates topics in the wrong account instead of failing.
Fix: the GetTopicAttributes handler's pre-store ARN validation (shared with the other ARN-taking reads): after the ARN parses and before the topic lookup, refuse an ARN whose account id is not the account this endpoint serves with HTTP 403, code 'InvalidClientTokenId' and exactly the message 'No account found for the given parameters'.
Traces: sns-refarch-s3ev-topic-lookup-mistakes#task#3.

=== Cause 2 of 10: the same foreign-account ARN refusal is missing on ListSubscriptionsByTopic ===
Fixture/step: sns-cnt-list-by-topic-reference-mistakes#task#5.
AWS: Smithy (com.amazonaws.sns#ListSubscriptionsByTopic) declares AuthorizationError (403), InvalidParameter (400), NotFound (404) and InternalError (500), and no error for an unresolvable account — recorded behavior only. The recording shows HTTP 403 with Error.Code = 'InvalidClientTokenId' and the same wholly fixed Error.Message 'No account found for the given parameters'.
Emulator: HTTP 404, Error.Code = 'NotFound', Error.Message = 'Topic does not exist'.
Why: as Cause 1 — the listing of a topic in another account reports an empty/absent resource rather than a credential fault, so a consumer that enumerates a topic's subscribers concludes the topic is gone.
Fix: the ListSubscriptionsByTopic handler's pre-store ARN validation. This is the SAME rule as Cause 1 on a different operation: it is listed separately because it is a separate wire step that must be re-replayed, and fixing it in a shared ARN pre-flight closes both.
Traces: sns-cnt-list-by-topic-reference-mistakes#task#5.

=== Cause 3 of 10: a TopicArn whose resource is not a legal TOPIC NAME is answered 404 NotFound instead of 400 InvalidParameter 'Invalid parameter: Topic Name' ===
Fixture/step: sns-refarch-s3ev-topic-lookup-mistakes#task#2.
AWS: Smithy: GetTopicAttributes.TopicArn is @required and targets the unconstrained string com.amazonaws.sns#topicARN, and the operation declares InvalidParameterException (awsQueryError InvalidParameter, httpError 400) — 'Indicates that a request parameter does not comply with the associated constraints'. Recorded behavior: an ARN whose resource part is not a name a topic could ever have is BAD INPUT, refused HTTP 400 with Error.Code = 'InvalidParameter' and Error.Message = 'Invalid parameter: Topic Name' — wholly fixed text naming the offending part of the ARN ('Topic Name'), with no value from the request echoed. The lookup never happens.
Emulator: HTTP 404 with Error.Code = 'NotFound' and Error.Message = 'Topic does not exist'. ops/get_topic_attributes.rs validates only ARN shape and region before the store, so an ARN carrying an unusable topic name reaches `store.topics.get(...)` and is reported as a topic that does not exist.
Why: the two answers demand opposite client behavior. 400/InvalidParameter says 'this request can never succeed, fix the string'; 404/NotFound says 'the topic is not there (yet)'. Code that retries or creates on NotFound loops forever on an ARN that is malformed, and nothing ever tells the caller the name itself is the problem.
Fix: the GetTopicAttributes handler's pre-store validation (`state::regional_arn_refusal` and the topic-name rule beside it): validate the ARN's resource against SNS's topic-name rule and refuse with 400 / InvalidParameter / 'Invalid parameter: Topic Name' before the store is consulted.
Traces: sns-refarch-s3ev-topic-lookup-mistakes#task#2.

=== Cause 4 of 10: a MISSING required member is reported as 'MalformedInput' with the runtime's constraint sentence instead of AWS's InvalidParameter 'Invalid parameter: <Member> Reason: no value for required parameter' ===
Fixture/step: sns-cnt-list-by-topic-reference-mistakes#task#4.
AWS: Smithy: ListSubscriptionsByTopic.TopicArn is @required, and the operation declares InvalidParameterException (awsQueryError code 'InvalidParameter', httpResponseCode 400). Recorded behavior: a request that carries no value at all for a required member is refused HTTP 400 with Error.Code = 'InvalidParameter' and Error.Message = 'Invalid parameter: TopicArn Reason: no value for required parameter'. Fixed text: 'Invalid parameter: ' … ' Reason: no value for required parameter'; the one variable part is the MEMBER NAME as the model spells it on the wire (TopicArn here).
Emulator: HTTP 400 is right, but Error.Code = 'MalformedInput' and Error.Message = "1 validation error detected: Value null at 'topicArn' failed to satisfy constraint: Member must not be null" — a generic validation-framework sentence that uses the lowerCamel member name and a code SNS never emits for this condition.
Why: 'InvalidParameter' is a modelled SNS error that SDK clients map to InvalidParameterException and that retry/classification layers key on; 'MalformedInput' is not in the operation's error set at all, so a generated client surfaces it as an unmodelled/unknown error. The message is also the only place the offending member is named, and it names a member spelling ('topicArn') that does not appear in the request the caller sent.
Fix: the SNS service's request-validation step (the deserialize → validate stage every op module documents), where a missing @required member is turned into a wire error: emit the modelled InvalidParameterException (400 / InvalidParameter) with the message 'Invalid parameter: <WireMemberName> Reason: no value for required parameter'.
Traces: sns-cnt-list-by-topic-reference-mistakes#task#4.

=== Cause 5 of 10: a CONSTRAINT violation on a member that IS present is coded 'MalformedInput' instead of AWS's 'ValidationError' ===
Fixture/step: sns-cnt-subscribe-endpoint-mistakes#task#3.
AWS: recorded behavior (the Smithy model declares no such error on Subscribe; the error belongs to the query protocol's shared validation layer, not to the operation's error set). The recording shows HTTP 400 with Error.Code = 'ValidationError' and the framework's own constraint sentence — the '<n> validation error(s) detected: Value … at '<member>' failed to satisfy constraint: …' form, whose fixed text is that frame and whose variable parts are the count, the offending value, the member path and the constraint named. Only the CODE diverged on this step: the status and the message already match, so the emulator reproduces AWS's sentence and then labels it with the wrong code.
Emulator: HTTP 400 with Error.Code = 'MalformedInput' and the matching message.
Why: this is the discriminator every client and log pipeline uses to tell a client-side constraint violation from a body the service could not parse at all. A client keyed on 'ValidationError' (the code AWS's query-protocol services emit for this class) sees an unknown code, and a request-replay or error-budget tool classifies a plain bad-argument call as a malformed request.
Fix: the shared query-protocol validation/error-serialization layer as mounted for SNS: serialize a constraint violation on a present member with Error.Code 'ValidationError' (Type 'Sender'), keeping the existing message frame. Note this is the SAME layer as Cause 4 but a DIFFERENT condition (present-but-invalid vs absent), and the two conditions take DIFFERENT codes — fixing one does not fix the other.
Traces: sns-cnt-subscribe-endpoint-mistakes#task#3.

=== Cause 6 of 10: the Subscribe bad-endpoint refusal substitutes a protocol-specific requirement phrase for AWS's 'Invalid parameter: Endpoint' ===
Fixture/step: sns-cnt-subscribe-endpoint-mistakes#task#2 (cluster of 3 steps: also sns-cnt-subscribe-endpoint-mistakes#task#4 and sns-snx-traj-raw-delivery-refused-on-lambda#task#3).
AWS: Smithy: Subscribe declares InvalidParameterException (awsQueryError InvalidParameter, httpError 400); Endpoint targets com.amazonaws.sns#Endpoint2, an unconstrained string whose documentation gives the form each protocol's endpoint takes. Recorded behavior: an Endpoint that is not in the form the requested Protocol requires is refused HTTP 400 / InvalidParameter with Error.Message = 'Invalid parameter: Endpoint' — fixed text that names the INPUT MEMBER and nothing else; no protocol name, no requirement phrase, and no value from the request. All three recorded steps in this cluster (two sqs endpoints and one lambda endpoint) expect the member-named form.
Emulator: status 400 and code InvalidParameter are right, but the message is 'Invalid parameter: SQS endpoint ARN' (and, for the lambda step, the matching 'Lambda endpoint ARN' wording). ops/subscribe.rs builds it as format!("Invalid parameter: {}", endpoint_requirement(&protocol)), and `endpoint_requirement` maps each protocol to its own phrase ('SQS endpoint ARN', 'Lambda endpoint ARN', 'Endpoint must match the specified protocol', 'Email address', 'PhoneNumber', …). The module comment admits only the lambda phrase was ever pinned to a recorded response; the corpus shows AWS using the member name for this condition.
Why: the message is the only machine-readable discriminator between the several conditions SNS groups under InvalidParameter on Subscribe, so a client or test matching AWS's text treats a legitimate endpoint refusal as an unrecognised failure, and a tool that maps the refusal back onto the input member cannot find the member it names.
Fix: the Subscribe handler's endpoint validation in ops/subscribe.rs — emit exactly 'Invalid parameter: Endpoint' for an endpoint that does not match the requested protocol, whatever the protocol, instead of interpolating `endpoint_requirement`.
Traces: sns-cnt-subscribe-endpoint-mistakes#task#2, sns-cnt-subscribe-endpoint-mistakes#task#4, sns-snx-traj-raw-delivery-refused-on-lambda#task#3.

=== Cause 7 of 10: the GetSubscriptionAttributes bad-ARN refusal is the emulator's own sentence, echoing the ARN, instead of AWS's 'Invalid parameter: SubscriptionId' ===
Fixture/step: sns-cnt-unsubscribe-drops-one-subscriber#task#4.
AWS: Smithy: GetSubscriptionAttributes.SubscriptionArn is @required and targets the unconstrained string com.amazonaws.sns#subscriptionARN; the operation declares InvalidParameterException (awsQueryError InvalidParameter, httpError 400) and NotFoundException (404). Recorded behavior: a SubscriptionArn that is not a subscription ARN (here a TOPIC ARN handed to this call) is refused HTTP 400 / InvalidParameter with Error.Message = 'Invalid parameter: SubscriptionId' — WHOLLY FIXED text naming the part of the ARN that is wrong, with NO part taken from the request: the submitted ARN is not echoed.
Emulator: status 400 and code InvalidParameter are right, but the message is "Invalid parameter: SubscriptionArn Reason: A subscription ARN names a topic and the subscription's id, so it has 7 elements: <the submitted ARN>" — a '<member> Reason: ' frame, an explanatory sentence of the emulator's own, and the caller's ARN interpolated into it.
Why: clients and tests branch on AWS's text to tell 'that is not a subscription ARN' from the other InvalidParameter conditions, and get an unrecognised sentence instead; log pipelines that aggregate error messages also see one distinct message per submitted ARN rather than one fixed class.
Fix: the GetSubscriptionAttributes handler's SubscriptionArn validation (and the shared subscription-ARN refusal helper it uses): emit exactly 'Invalid parameter: SubscriptionId' with no member frame and no echoed value.
Traces: sns-cnt-unsubscribe-drops-one-subscriber#task#4.

=== Cause 8 of 10: re-subscribing the same endpoint with DIFFERENT attributes is accepted (200) instead of refused 400 InvalidParameter 'Invalid parameter: Attributes Reason: Subscription already exists with different attributes' ===
Fixture/step: sns-cnt-resubscribe-does-not-inflate-count#task#4.
AWS: Smithy: Subscribe declares InvalidParameterException (awsQueryError InvalidParameter, httpError 400). Recorded behavior: Subscribe is idempotent only for an IDENTICAL request — re-subscribing the same (TopicArn, Protocol, Endpoint) triple while carrying a different attribute value is refused HTTP 400 with ErrorResponse.Error.Type = 'Sender', Error.Code = 'InvalidParameter' and Error.Message = 'Invalid parameter: Attributes Reason: Subscription already exists with different attributes' — wholly fixed text, nothing from the request. The refusal is total: the existing subscription keeps the attributes it was created with.
Emulator: the call is accepted with HTTP 200 and no ErrorResponse at all. ops/subscribe.rs finds the existing subscription by (protocol, endpoint) and then deliberately MERGES the request's attributes into the stored record ('The attributes a re-subscribe carries are APPLIED to the subscription that already exists'), so the conflicting call succeeds and silently rewrites a live subscription's configuration.
Why: this is the rule that stops a re-run of an idempotent provisioning step from flipping a live subscription's delivery configuration under a running consumer. Against the emulator a second Subscribe can turn raw delivery, a filter policy or a delivery policy on or off with a 200, and the client gets no signal that its two code paths disagree.
Fix: the Subscribe handler's existing-subscription resolution in ops/subscribe.rs — return the existing SubscriptionArn only when the requested Attributes are identical to the stored ones, and otherwise refuse with 400 / InvalidParameter / the message above without touching the stored record. The behavioral twin of this cause is Cause 4 of 20261102T093047Z-sns-sqs-4-net-new-root-causes-an.md, filed under the `sns__sqs` token and carrying NO wire replay key, so it does not cover this step.
Traces: sns-cnt-resubscribe-does-not-inflate-count#task#4.

=== Cause 9 of 10: the GetTopicAttributes SubscriptionsDeleted counter reports 1 where AWS reports 0 after an unsubscribe ===
Fixture/step: sns-cnt-topic-subscription-counters-settle#task#7.
AWS: Smithy: GetTopicAttributesResponse.Attributes documents 'SubscriptionsDeleted – The number of deleted subscriptions for the topic' beside SubscriptionsConfirmed and SubscriptionsPending, all reported as decimal strings. Recorded behavior: at this step of the counters fixture — after the topic's subscription has been removed and the confirmed/pending counts have settled (those entries match) — AWS still reports '0' for the entry the emulator fills with the deleted count. Hypothesis: AWS's deleted counter is not advanced synchronously with the Unsubscribe call that is being observed here, so a read taken straight after the unsubscribe still sees 0.
Emulator: that entry reads '1'. The handler reports `topic.subscriptions_deleted` (ops/get_topic_attributes.rs, the SUBSCRIPTIONS_DELETED_ATTRIBUTE entry) from the stored per-topic counter (state::TopicRecord::subscriptions_deleted), which is incremented by Unsubscribe and read back immediately. In the attribute order the emulator itself reports (state::REPORTED_TOPIC_ATTRIBUTE_ORDER: Policy, Owner, SubscriptionsPending, KmsMasterKeyId, TopicArn, EffectiveDeliveryPolicy, SubscriptionsConfirmed, DisplayName, SubscriptionsDeleted) this is the last entry, which is the entry[7] the replay reports for a topic with no KMS key.
Why: a client that polls GetTopicAttributes to decide when a topic's subscriber bookkeeping has settled — or that asserts a topic is quiescent before deleting it — reads a different number from AWS for the same sequence of calls, so drift detection and convergence loops keyed on the counter triple disagree with the real service.
Fix: the GetTopicAttributes attribute assembly together with the state that feeds it (TopicRecord::subscriptions_deleted and the Unsubscribe path that increments it): report the deleted count AWS reports for this sequence rather than a counter advanced synchronously by Unsubscribe.
Traces: sns-cnt-topic-subscription-counters-settle#task#7.

=== Cause 10 of 10: ListSubscriptions answers without a NextToken where AWS's response carries one ===
Fixture/step: sns-cnt-list-subscriptions-token-refused-then-listed#task#4.
AWS: Smithy: ListSubscriptions is @paginated with inputToken NextToken / outputToken NextToken / items Subscriptions, and the output member NextToken ('nextToken') is documented as the token to pass to the next call. Recorded behavior: at this step — an account-wide listing taken straight after a bad token was correctly refused — the response carries a NextToken (the corpus asserts its PRESENCE and shape, not its value, since the token is opaque and per-run). The emulator's response has no NextToken member at all.
Emulator: ops/list_subscriptions.rs collects every subscription of the region and calls `state::paginate(all, input.next_token, state::PAGE_SIZE)` with PAGE_SIZE = 100, so a NextToken is produced ONLY when the region holds more than 100 subscription records; for any smaller store the member is omitted. Hypothesis: AWS's account-wide subscription listing hands back a continuation token whenever a further page may exist, independently of whether this page filled.
Why: a client that drives the documented pagination loop (call, read NextToken, repeat until absent) terminates after one page against the emulator but not against AWS, and a client that asserts the paginated contract — a token it can hand back to the same operation — has nothing to hand back, so the token-refused/token-accepted pair of paths cannot both be exercised.
Fix: the ListSubscriptions handler's pagination (ops/list_subscriptions.rs and the shared state::paginate it calls): report a NextToken for the account-wide listing on AWS's terms rather than only above the 100-record page size. Note: only the PRESENCE of the member is a requirement — the token's bytes are per-run and are not asserted.
Traces: sns-cnt-list-subscriptions-token-refused-then-listed#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, /__admin/health 200): 37 fixtures replayed, 293 steps, 275 passed, 17 mismatched, 0 errored, 1 blocked, score 93.9%. 15 divergence clusters, all service `sns`; 5 are owned by 20261030T200727Z-sns-5-wire-divergences-the-opt-out-check.md and excluded here. The remaining 10 clusters (12 steps) are the 10 causes:

Cause 1 — sns-refarch-s3ev-topic-lookup-mistakes#task#3, GetTopicAttributes:
  status: expected 403, got 404
  $.ErrorResponse.Error.Code: expected 'InvalidClientTokenId', got 'NotFound'
  $.ErrorResponse.Error.Message: expected 'No account found for the given parameters', got 'Topic does not exist'
  (divergence_key service=sns|GetTopicAttributes|status=403->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status)

Cause 2 — sns-cnt-list-by-topic-reference-mistakes#task#5, ListSubscriptionsByTopic:
  status: expected 403, got 404
  $.ErrorResponse.Error.Code: expected 'InvalidClientTokenId', got 'NotFound'
  $.ErrorResponse.Error.Message: expected 'No account found for the given parameters', got 'Topic does not exist'
  (divergence_key service=sns|ListSubscriptionsByTopic|status=403->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status)

Cause 3 — sns-refarch-s3ev-topic-lookup-mistakes#task#2, GetTopicAttributes:
  status: expected 400, got 404
  $.ErrorResponse.Error.Code: expected 'InvalidParameter', got 'NotFound'
  $.ErrorResponse.Error.Message: expected 'Invalid parameter: Topic Name', got 'Topic does not exist'
  (divergence_key service=sns|GetTopicAttributes|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status)

Cause 4 — sns-cnt-list-by-topic-reference-mistakes#task#4, ListSubscriptionsByTopic (status 400 matched):
  $.ErrorResponse.Error.Code: expected 'InvalidParameter', got 'MalformedInput'
  $.ErrorResponse.Error.Message: expected 'Invalid parameter: TopicArn Reason: no value for required parameter', got "1 validation error detected: Value null at 'topicArn' failed to satisfy constraint: Member must not be null"
  (divergence_key service=sns|ListSubscriptionsByTopic|status=400->400|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message)

Cause 5 — sns-cnt-subscribe-endpoint-mistakes#task#3, Subscribe (status 400 AND Error.Message matched; only the code diverged):
  $.ErrorResponse.Error.Code: expected 'ValidationError', got 'MalformedInput'
  (divergence_key service=sns|Subscribe|status=400->400|keys=$.ErrorResponse.Error.Code)

Cause 6 — sns-cnt-subscribe-endpoint-mistakes#task#2, Subscribe (status 400 and code InvalidParameter matched), cluster of 3 steps, also sns-cnt-subscribe-endpoint-mistakes#task#4 and sns-snx-traj-raw-delivery-refused-on-lambda#task#3:
  $.ErrorResponse.Error.Message: expected 'Invalid parameter: Endpoint', got 'Invalid parameter: SQS endpoint ARN'
  (divergence_key service=sns|Subscribe|status=400->400|keys=$.ErrorResponse.Error.Message; affected_count=3)

Cause 7 — sns-cnt-unsubscribe-drops-one-subscriber#task#4, GetSubscriptionAttributes (status 400 and code matched):
  $.ErrorResponse.Error.Message: expected 'Invalid parameter: SubscriptionId', got "Invalid parameter: SubscriptionArn Reason: A subscription ARN names a topic and the subscription's id, so it has 7 elements: arn:aws:sns:us-west-2:000000000000:sim2real-probe-cnt-unsub-9b3e17c4"
  (the trailing ARN is this recording's input, echoed by the emulator; AWS's message is the fixed sentence 'Invalid parameter: SubscriptionId' with nothing interpolated)

Cause 8 — sns-cnt-resubscribe-does-not-inflate-count#task#4, Subscribe:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameter', 'Message': 'Invalid parameter: Attributes Reason: Subscription already exists with different attributes'}, 'RequestId': <not asserted>}, got None)
  (divergence_key service=sns|Subscribe|status=400->200|keys=$.ErrorResponse,status)

Cause 9 — sns-cnt-topic-subscription-counters-settle#task#7, GetTopicAttributes (status 200 in both; all other attribute entries matched):
  $.GetTopicAttributesResponse.GetTopicAttributesResult.Attributes.entry[7].value: expected '0', got '1'
  (entry[7] is the last entry of state::REPORTED_TOPIC_ATTRIBUTE_ORDER for a topic with no KmsMasterKeyId, i.e. SubscriptionsDeleted; the SubscriptionsConfirmed and SubscriptionsPending entries matched)

Cause 10 — sns-cnt-list-subscriptions-token-refused-then-listed#task#4, ListSubscriptions (status 200 in both):
  $.ListSubscriptionsResponse.ListSubscriptionsResult.NextToken: missing (expected '<shaped-not-asserted>', got None)
  (presence/shape of the token is asserted, not its bytes)

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1 and re-replay exactly the fixture#step pairs named here (Issue-Trace/Issue-Step plus every token in Issue-Trace-Ids).
3. Per cause, by hand (query protocol over the SNS endpoint):
   - Cause 1 (sns-refarch-s3ev-topic-lookup-mistakes#task#3): GetTopicAttributes with a well-formed TopicArn whose ACCOUNT id is not the served account — expect 403 / InvalidClientTokenId / 'No account found for the given parameters'; observe 404 / NotFound / 'Topic does not exist'.
   - Cause 2 (sns-cnt-list-by-topic-reference-mistakes#task#5): the same ARN through ListSubscriptionsByTopic — same expectation, same observed 404.
   - Cause 3 (sns-refarch-s3ev-topic-lookup-mistakes#task#2): GetTopicAttributes with an ARN whose resource is not a legal topic name — expect 400 / InvalidParameter / 'Invalid parameter: Topic Name'; observe 404 / NotFound.
   - Cause 4 (sns-cnt-list-by-topic-reference-mistakes#task#4): ListSubscriptionsByTopic with NO TopicArn parameter at all — expect 400 / InvalidParameter / 'Invalid parameter: TopicArn Reason: no value for required parameter'; observe code MalformedInput and the '1 validation error detected' sentence.
   - Cause 5 (sns-cnt-subscribe-endpoint-mistakes#task#3): the fixture's Subscribe whose member violates a constraint — the message already matches; read Error.Code only: expect 'ValidationError', observe 'MalformedInput'.
   - Cause 6 (sns-cnt-subscribe-endpoint-mistakes#task#2 and #task#4, sns-snx-traj-raw-delivery-refused-on-lambda#task#3): Subscribe(Protocol='sqs', Endpoint=<not an SQS queue ARN>) and the lambda equivalent — read Error.Message: expect 'Invalid parameter: Endpoint', observe the protocol-specific phrase ('Invalid parameter: SQS endpoint ARN' / 'Lambda endpoint ARN').
   - Cause 7 (sns-cnt-unsubscribe-drops-one-subscriber#task#4): GetSubscriptionAttributes(SubscriptionArn=<a TOPIC ARN>) — expect exactly 'Invalid parameter: SubscriptionId'; observe the emulator's 'SubscriptionArn Reason: …7 elements: <arn>' sentence.
   - Cause 8 (sns-cnt-resubscribe-does-not-inflate-count#task#4): CreateTopic; Subscribe an endpoint with Attributes={...}; Subscribe the SAME topic/protocol/endpoint with a DIFFERENT attribute value — expect 400 Sender/InvalidParameter/'Invalid parameter: Attributes Reason: Subscription already exists with different attributes' and the stored attributes unchanged; observe 200 and the stored attributes rewritten.
   - Cause 9 (sns-cnt-topic-subscription-counters-settle#task#7): CreateTopic; Subscribe; Unsubscribe; GetTopicAttributes and read the SubscriptionsDeleted entry (last entry of the reported order for a non-encrypted topic) — AWS still reports '0' at this point; the emulator reports '1'.
   - Cause 10 (sns-cnt-list-subscriptions-token-refused-then-listed#task#4): ListSubscriptions with a bogus NextToken (correctly refused), then ListSubscriptions with no token — AWS's response carries a NextToken member; the emulator omits it unless the region holds more than 100 subscriptions.
