Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event events.create-event-bus-reserved-and-malformed-names-rejected events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule|run#23|AssertionError|assert mine, (
Issue-Key: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event|_wait_live#15|AssertionError|raise AssertionError(f"none of {len(sent)} liveness event(s) reached the endpoint within 180s")
Issue-Key: events.create-event-bus-reserved-and-malformed-names-rejected|run#9|AssertionError|assert message == expected, f"CreateEventBus({name[:60]!r}...) message {message!r}, expected {expected!r}"
Issue-Key: events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern|run#16|AssertionError|assert got == refusal, f"a {_LIMIT + 1}-character pattern gave {got!r}"
Issue-Key: events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged|run#10|AssertionError|assert (code, message) == ("ValidationException", expected), f"TagResource aws: key gave {code!r} {message!r}"
Issue-Commit: 3c33b9e3725ab606ee2a23453a6abf264aabc19e

# events: 5 net-new causes — a Lambda target's handler error produces no OnFailure record, a rate-limited API destination delivers nothing, and three refusal messages (long bus name, over-limit EventPattern, reserved aws: tag keys) are emulator-authored

> _Note: this issue was identified against the code base at commit `3c33b9e3725ab606ee2a23453a6abf264aabc19e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Five independent EventBridge defects, each grounded in a failing check listed under the `events` heading. None of the five is carried by an open issue's replay key; related open issues are named per cause for context only.

Cause 1 of 5: a Lambda target whose handler raises produces no asynchronous-invoke failure record at all.
AWS: EventBridge invokes a Lambda target asynchronously. The handler's exception is Lambda's to retry: with PutFunctionEventInvokeConfig MaximumRetryAttempts=1 the function is invoked 1+1 times and Lambda then writes the OnFailure destination record, whose requestContext.condition is `RetriesExhausted`, requestContext.approximateInvokeCount is 1+MaximumRetryAttempts, requestPayload is the published EventBridge envelope (source and detail as sent), responseContext.functionError is `Unhandled`, and responsePayload carries the handler's errorType/errorMessage. The rule's own DeadLetterConfig queue stays empty for a handler error - the rule's delivery succeeded. (Recorded behavior plus https://docs.aws.amazon.com/lambda/latest/dg/invocation-async.html; Smithy: Lambda PutFunctionEventInvokeConfig binds MaximumRetryAttempts and DestinationConfig.OnFailure.)
Emulator: within 300 s and across 5 republished events, zero records arrive on the OnFailure destination queue (`no OnFailure destination record arrived within 300.0s for any of the 5 event(s) sent; 0 other record(s)`), so neither the retry count, nor the envelope, nor the error payload, nor the "never dead-lettered by the rule" half of the contract is observable.
Why: a client cannot observe or alert on a failing event-driven function: the standard failure sink (OnSuccess/OnFailure destinations) is silent, and there is no way to distinguish "event never delivered" from "handler failed".
Fix: the Lambda asynchronous-invoke path (its retry loop and the DestinationConfig OnFailure writer) and the EventBridge Lambda-target invocation that feeds it. Related open issues 20261107T042814Z-dynamodb-events-lambda-lambda-asynchronous-invoke-destinations-never.md and 20261107T042814Z-events-api-destination-dead-letter-attributes-and-the.md describe neighbouring symptoms and do not carry this check.
Traces: events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule.

Cause 2 of 5: an API destination target with an invocation rate limit delivers no event whatsoever.
AWS: a rule target that is an API destination ARN (with a RoleArn granting events:InvokeApiDestination and an AUTHORIZED API_KEY connection) delivers each matched event to the destination's InvocationEndpoint, injecting the connection's credential header; InvocationRateLimitPerSecond only paces the deliveries (a burst above the limit is deferred, not dropped and not dead-lettered). (Recorded behavior plus https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-api-destinations.html; Smithy: CreateApiDestination.InvocationRateLimitPerSecond, DescribeApiDestination echoes it.)
Emulator: DescribeApiDestination reads the rate back as 1 correctly, but no event ever reaches the endpoint - 9 liveness events published over 180 s produce zero requests at the destination (`none of 9 liveness event(s) reached the endpoint within 180s`), so the pacing contract cannot even be measured.
Why: any integration that leaves the bus over HTTP is silently dead; the caller sees FailedEntryCount 0 from PutEvents and a configured, AUTHORIZED destination that never receives anything.
Fix: the EventBridge api-destination target delivery path (connection credential injection, HTTP dispatch) and its rate-limit scheduler. Related open issue 20261102T091831Z-events-api-destinations-never-deliver-or-dead-letter.md reports the same family under other checks and does not carry this one.
Traces: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event.

Cause 3 of 5: CreateEventBus's length-constraint message truncates the echoed value instead of reproducing it.
AWS: Smithy: CreateEventBus.Name targets EventBusName with @length min 1 max 256 and pattern ^[/\.\-_A-Za-z0-9]+$. A 257-character name is refused 400 ValidationException with the SDK constraint-violation template `1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 256` - fixed text is `1 validation error detected: Value '`, `' at 'name' failed to satisfy constraint: Member must have length less than or equal to 256`; the only variable part is the rejected value, echoed VERBATIM and in full from the request.
Emulator: the same 400 ValidationException, but the value slot is cut short and annotated with a byte count the template has no room for: `Value '<first ~250 chars>... [257 bytes]' at 'name' failed to satisfy constraint: Member must have length less than or equal to 256`.
Why: the echoed value is how a caller identifies which of several submitted names was rejected; a truncated, re-annotated value breaks exact-message comparisons and hides the offending input. The same renderer serves every constrained member, so the defect is not specific to event buses.
Fix: the constraint-violation message renderer used by the eventbridge crate's input validation - echo the rejected value verbatim with no truncation and no `[N bytes]` annotation.
Traces: events.create-event-bus-reserved-and-malformed-names-rejected.

Cause 4 of 5: PutRule's over-limit EventPattern refusal message is emulator-authored.
AWS: an EventPattern longer than the 2048-character service quota is refused 400 InvalidEventPatternException with message `Parameter EventPattern for rule <ruleName> exceeds limit of 2048.` - fixed text `Parameter EventPattern for rule `, ` exceeds limit of `, `.`; variable parts are the request's rule Name and the quota (2048). The measurement is of the pattern text AS SENT (whitespace counted), so a pretty-printed pattern whose compact form would fit is refused too, and a refused PutRule leaves the stored pattern unchanged. (Smithy: PutRule binds InvalidEventPatternException as a client 400; the modelled EventPattern @length max of 4096 is not the enforced quota, which is 2048.)
Emulator: the correct code with a different message: `Event pattern is not valid. Reason: Event pattern is too long.` - it names neither the rule nor the limit and reuses the generic invalid-pattern sentence.
Why: the message is the only payload that tells the caller which parameter and which quota were exceeded; a client surfacing or matching it cannot distinguish a size refusal from a syntax refusal.
Fix: the EventPattern size validation shared by PutRule (and the other pattern-accepting operations) - render AWS's parameter/limit sentence and measure the request text as sent.
Traces: events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern.

Cause 5 of 5: the reserved `aws:` tag-key refusal on TagResource/UntagResource is worded by the emulator.
AWS: Smithy: TagResource/UntagResource take ResourceARN plus Tags/TagKeys. A request containing a tag key in the reserved `aws:` namespace is refused 400 ValidationException with the fixed sentence `Add/Remove system tags is not allowed.` - entirely fixed text, no variable parts; the refusal is atomic (a request mixing a legal and a reserved key writes neither) and leaves the resource's tags unchanged.
Emulator: the same 400 ValidationException with the emulator's own sentence `Tag keys starting with 'aws:' are reserved for AWS use.`
Why: a caller or test keyed to AWS's fixed sentence never matches, and because this is the first assertion the check reaches, the atomicity of a mixed legal/reserved TagResource and the UntagResource variant remain unverified behind it.
Fix: the reserved-tag-key validation shared by the eventbridge TagResource/UntagResource handlers - emit AWS's fixed sentence.
Traces: events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged.

## Evidence
Cause 1: `AssertionError: no OnFailure destination record arrived within 300.0s for any of the 5 event(s) sent; 0 other record(s)` (events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule). Expected one record with requestContext.condition == 'RetriesExhausted', requestContext.approximateInvokeCount == 2 (MaximumRetryAttempts=1), responseContext.functionError == 'Unhandled', responsePayload.errorType == 'RuntimeError', and an empty rule DLQ.

Cause 2: `AssertionError: none of 9 liveness event(s) reached the endpoint within 180s` (events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event), after DescribeApiDestination correctly reported InvocationRateLimitPerSecond == 1 and PutTargets returned FailedEntryCount 0.

Cause 3: CreateEventBus with a 257-character name -
  expected: "1 validation error detected: Value '<the full 257-character name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 256"
  got:      "1 validation error detected: Value '<name truncated>... [257 bytes]' at 'name' failed to satisfy constraint: Member must have length less than or equal to 256"
Both 400 ValidationException. The sibling cases in the same check (`aws.`-prefixed and `/`-containing names) pass.

Cause 4: PutRule with a 2049-character EventPattern -
  expected: ('InvalidEventPatternException', 'Parameter EventPattern for rule <ruleName> exceeds limit of 2048.')
  got:      ('InvalidEventPatternException', 'Event pattern is not valid. Reason: Event pattern is too long.')
The 2048-character pattern is accepted and stored verbatim, so only the refusal message diverges.

Cause 5: TagResource(ResourceARN=<bus arn>, Tags=[{Key: 'aws:<x>', Value: 'x'}]) -
  expected: ('ValidationException', 'Add/Remove system tags is not allowed.')
  got:      ('ValidationException', "Tag keys starting with 'aws:' are reserved for AWS use.")

## Steps to Reproduce
1. Replay the six-check list in trace_ids.
2. Cause 1: create a python3.12 function whose handler raises RuntimeError; PutFunctionEventInvokeConfig with MaximumRetryAttempts=1 and DestinationConfig.OnFailure pointing at an SQS queue; put a rule on a custom bus with that function as target (DeadLetterConfig set, queue policy admitting events.amazonaws.com); PutEvents a matching event and poll the destination queue - expect one RetriesExhausted record with approximateInvokeCount 2 and an empty rule DLQ.
3. Cause 2: create an API_KEY connection, an API destination with InvocationRateLimitPerSecond=1 pointing at an HTTPS endpoint you can observe, a role granting events:InvokeApiDestination, and a rule target on that destination ARN; PutEvents and watch the endpoint - expect every event to arrive, paced to ~1/s.
4. Cause 3: CreateEventBus(Name=<257 characters>) and compare Error.Message against the constraint template with the value echoed in full.
5. Cause 4: PutRule with an EventPattern of 2049 characters (and separately a pretty-printed pattern over 2048 whose compact form fits) and compare Error.Code/Error.Message against 'Parameter EventPattern for rule <name> exceeds limit of 2048.'; then DescribeRule to confirm the previously stored pattern is unchanged.
6. Cause 5: on a custom bus ARN, call TagResource with a key 'aws:<x>', then TagResource with one legal plus one 'aws:' key, then UntagResource with TagKeys=['aws:<x>']; each must be 400 ValidationException 'Add/Remove system tags is not allowed.' with ListTagsForResource unchanged.
