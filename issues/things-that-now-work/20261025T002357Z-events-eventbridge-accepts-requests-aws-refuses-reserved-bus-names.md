Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.create-event-bus-reserved-and-malformed-names-rejected events.event-bus-dead-letter-config-rejects-non-sqs-arns-and-keeps-prior-config events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern events.put-rule-invalid-schedule-expression-rejected events.put-rule-schedule-expression-grammar-rejection events.schedule-rule-on-custom-bus-rejected events.input-transformer-unsupported-input-path-forms-rejected-and-targets-unchanged events.put-targets-omitted-role-arn-rejected-leaves-prior-target-unchanged events.bus-target-without-role-arn-rejected-in-both-regions-and-the-rule-keeps-its-targets events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged events.oauth-connection-non-https-authorization-endpoint-rejected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.create-event-bus-reserved-and-malformed-names-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.event-bus-dead-letter-config-rejects-non-sqs-arns-and-keeps-prior-config|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern|_error#15|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Key: events.put-rule-invalid-schedule-expression-rejected|_client_error#8|AssertionError|raise AssertionError("expected a ClientError, but the call SUCCEEDED")
Issue-Key: events.put-rule-schedule-expression-grammar-rejection|_error_code#2|AssertionError|raise AssertionError("expected a ClientError, but the call SUCCEEDED")
Issue-Key: events.schedule-rule-on-custom-bus-rejected|_client_error#11|AssertionError|raise AssertionError(f"expected PutRule to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: events.input-transformer-unsupported-input-path-forms-rejected-and-targets-unchanged|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.put-targets-omitted-role-arn-rejected-leaves-prior-target-unchanged|_attempt_put_targets#12|AssertionError|raise AssertionError(
Issue-Key: events.bus-target-without-role-arn-rejected-in-both-regions-and-the-rule-keeps-its-targets|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it")
Issue-Key: events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.oauth-connection-non-https-authorization-endpoint-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge accepts requests AWS refuses: reserved bus names, bad schedule expressions, oversized patterns, non-SQS DLQ ARNs, unsupported InputTransformer paths, reserved tag keys, missing target RoleArn and non-HTTPS OAuth endpoints (10 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Ten independent input-validation rules are not enforced; in each case the emulator answers HTTP 2xx and creates/updates the resource where AWS refuses the request and leaves prior state untouched.

Cause 1 of 10 — CreateEventBus accepts a reserved/malformed bus name.
AWS: `CreateEventBus.Name` is `EventBusName` (length 1-256, pattern `^[/\.\-_A-Za-z0-9]+$`); names in the reserved `aws.`-prefixed namespace are refused (only a partner bus named via EventSourceName may use `aws.partner/...`).
Emulator: returns 200 with `EventBusArn: arn:aws:events:<region>:<acct>:event-bus/aws.<name>`.
Why: a client can squat on the reserved namespace and shadow service buses.
Fix: name validation in services/service-com-amazonaws-eventbridge-awsevents/src/ops/create_event_bus.rs.
Traces: events.create-event-bus-reserved-and-malformed-names-rejected.

Cause 2 of 10 — DeadLetterConfig accepts a non-SQS ARN.
AWS: Smithy documents `DeadLetterConfig.Arn` as "The ARN of the SQS queue specified as the target for the dead-letter queue"; an SNS topic ARN is refused and the bus keeps its prior DeadLetterConfig.
Emulator: returns 200 and stores `DeadLetterConfig: {'Arn': 'arn:aws:sns:...'}`.
Why: undeliverable events are routed to a destination EventBridge can never write to, and the misconfiguration is discovered only when events are lost.
Fix: DeadLetterConfig ARN validation in create_event_bus.rs / update_event_bus.rs.
Traces: events.event-bus-dead-letter-config-rejects-non-sqs-arns-and-keeps-prior-config.

Cause 3 of 10 — PutRule accepts an over-long EventPattern.
AWS: recorded behavior — a pattern longer than the service's documented 2048-character event-pattern limit (whitespace counted) is refused and the rule keeps its previous pattern. (The Smithy `EventPattern` member only carries @length max 4096, so the enforced limit is the recorded one.)
Emulator: the call succeeds.
Why: a rule can be created that AWS would reject, so a template that deploys here fails in production.
Fix: EventPattern length validation in .../src/ops/put_rule.rs.
Traces: events.event-pattern-over-2048-characters-rejected-counting-whitespace-and-the-rule-keeps-its-pattern.

Cause 4 of 10 — PutRule accepts an invalid ScheduleExpression.
AWS: `ScheduleExpression` must be `rate(...)`/`cron(...)` in the documented grammar; anything else is refused with a 400 client error and no rule is created.
Emulator: "expected a ClientError, but the call SUCCEEDED".
Fix: ScheduleExpression parsing/validation in put_rule.rs.
Traces: events.put-rule-invalid-schedule-expression-rejected.

Cause 5 of 10 — the ScheduleExpression grammar itself is not checked field by field.
AWS: cron(...) with the wrong field count, out-of-range values, or both day-of-month and day-of-week specified is refused.
Emulator: "expected a ClientError, but the call SUCCEEDED".
Fix: the same ScheduleExpression parser.
Traces: events.put-rule-schedule-expression-grammar-rejection.

Cause 6 of 10 — PutRule accepts a ScheduleExpression on a custom bus.
AWS: scheduled rules exist only on the default bus; PutRule with a ScheduleExpression and a custom EventBusName is refused.
Emulator: returns 200 with `RuleArn: arn:aws:events:...:rule/<custom-bus>/<rule>`.
Why: the client believes it has a working schedule on a custom bus; it will never fire anywhere.
Fix: the bus/schedule compatibility check in put_rule.rs.
Traces: events.schedule-rule-on-custom-bus-rejected.

Cause 7 of 10 — PutTargets accepts unsupported InputTransformer InputPath forms.
AWS: InputPathsMap values must be the supported JSON-path subset; unsupported forms are refused and the rule's targets are unchanged.
Emulator: returns 200 with FailedEntryCount 0 and FailedEntries [].
Fix: InputTransformer validation in .../src/ops/put_targets.rs.
Traces: events.input-transformer-unsupported-input-path-forms-rejected-and-targets-unchanged.

Cause 8 of 10 — PutTargets accepts a target that requires RoleArn without one.
AWS: target types that EventBridge must assume a role to invoke (a Step Functions state machine, a cross-account/cross-region event bus) require RoleArn; omitting it is refused and any prior target for that Id is left in place.
Emulator: accepts `{'Id': 't1', 'Arn': 'arn:aws:states:...:stateMachine:<sm>'}` with no RoleArn, answering FailedEntryCount 0 / FailedEntries [].
Why: the target is stored and then can never be invoked; a client sees a healthy rule that silently delivers nothing.
Fix: per-target-type required-parameter validation in put_targets.rs.
Traces: events.put-targets-omitted-role-arn-rejected-leaves-prior-target-unchanged, events.bus-target-without-role-arn-rejected-in-both-regions-and-the-rule-keeps-its-targets.

Cause 9 of 10 — TagResource accepts reserved system tag keys.
AWS: tag keys in the reserved `aws:` namespace cannot be set by a customer; the call is refused and the resource's tags are unchanged.
Emulator: returns 200 with an empty body.
Fix: tag-key validation in .../src/ops/tag_resource.rs.
Traces: events.system-tag-keys-rejected-on-event-bus-and-tags-unchanged.

Cause 10 of 10 — CreateConnection accepts a non-HTTPS OAuth AuthorizationEndpoint.
AWS: the OAuth AuthorizationEndpoint is an `HttpsEndpoint`; an `http://` endpoint is refused and no connection is created.
Emulator: returns 200 with `ConnectionArn`, `ConnectionState: 'CREATING'`, CreationTime and LastModifiedTime.
Fix: AuthParameters endpoint validation in .../src/ops/create_connection.rs.
Traces: events.oauth-connection-non-https-authorization-endpoint-rejected.

## Evidence
Each replayed call returned 2xx where a 4xx refusal was required:
- CreateEventBus(Name="aws.<name>") -> 200 `{'EventBusArn': 'arn:aws:events:us-west-2:...:event-bus/aws.<name>', ... 'HTTPStatusCode': 200}`.
- UpdateEventBus DeadLetterConfig with an SNS ARN -> 200 `{'Arn': '...event-bus/<bus>', 'Name': '<bus>', 'DeadLetterConfig': {'Arn': 'arn:aws:sns:us-west-2:...:<topic>'}, 'HTTPStatusCode': 200}`.
- PutRule with a >2048-char EventPattern -> "the call succeeded; real AWS refuses it".
- PutRule with an invalid/ungrammatical ScheduleExpression -> "expected a ClientError, but the call SUCCEEDED" (both schedule checks).
- PutRule with ScheduleExpression on a custom bus -> 200 `{'RuleArn': 'arn:aws:events:...:rule/<bus>/<rule>', 'HTTPStatusCode': 200}`.
- PutTargets with unsupported InputTransformer InputPath -> 200 `{'FailedEntryCount': 0, 'FailedEntries': []}`.
- PutTargets missing RoleArn -> "PutTargets ACCEPTED a target missing a required RoleArn: {'Id': 't1', 'Arn': 'arn:aws:states:us-west-2:...:stateMachine:<sm>'}; response was {'FailedEntryCount': 0, 'FailedEntries': []}"; the cross-region bus-target variant reports "the call succeeded; real AWS refuses it".
- TagResource with an `aws:`-prefixed key -> 200 with an empty body.
- CreateConnection with an http:// OAuth AuthorizationEndpoint -> 200 `{'ConnectionArn': '...', 'ConnectionState': 'CREATING', ...}`.

## Steps to Reproduce
1. CreateEventBus(Name="aws.mybus") -> observe 200.
2. CreateEventBus(Name=b); UpdateEventBus(Name=b, DeadLetterConfig={'Arn': <SNS topic arn>}) -> observe 200 and the SNS ARN stored.
3. PutRule(Name=r, EventPattern=<a syntactically valid pattern padded with whitespace past 2048 chars>) -> observe 200.
4. PutRule(Name=r2, ScheduleExpression="rate(nonsense)") and PutRule(Name=r3, ScheduleExpression="cron(1 2 3)") -> observe 200.
5. PutRule(Name=r4, EventBusName=b, ScheduleExpression="rate(5 minutes)") -> observe 200.
6. PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<state machine arn>}]) with no RoleArn -> observe FailedEntryCount 0.
7. PutTargets with InputTransformer.InputPathsMap using an unsupported path form -> observe FailedEntryCount 0.
8. TagResource(ResourceARN=<bus arn>, Tags=[{Key:'aws:foo', Value:'v'}]) -> observe 200.
9. CreateConnection(Name=c, AuthorizationType='OAUTH_CLIENT_CREDENTIALS', AuthParameters with AuthorizationEndpoint='http://example.com/token') -> observe 200.
