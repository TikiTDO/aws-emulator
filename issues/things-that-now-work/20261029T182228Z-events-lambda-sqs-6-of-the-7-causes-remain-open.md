Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout events__lambda__sqs.eventbridge-fanout-lambda-and-sqs events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda events__lambda__sqs.cli-delete-event-source-mapping-stops-polling events__lambda__sqs.lambda-esm-dlq-on-repeated-failure events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order events__lambda__sqs.sqs-lambda-event-source-mapping-basic events__lambda__sqs.sqs-to-lambda-event-source-mapping events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target
Issue-Key: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda|_reemit_until_delivered#13|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution|_reemit_until_delivered#17|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout|_emit_until_delivered#11|AssertionError|raise AssertionError(f"event never delivered to: {missing} after {attempts} attempts")
Issue-Key: events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative|_drain_queue_for#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke|_drain_queue_for#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout|_drain_queue_for#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.eventbridge-fanout-lambda-and-sqs|_collect_messages#15|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke|_collect_messages#20|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke|_drain_queue#22|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload|_drain_queue#7|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb|run#38|Timeout|time.sleep(1)
Issue-Key: events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda|_drain_queue#7|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain|_wait_for_messages#15|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions|_drain#17|Timeout|return out
Issue-Key: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda|_drain_queue_for#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.cli-delete-event-source-mapping-stops-polling|_drain_queue_for#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.lambda-esm-dlq-on-repeated-failure|_wait_for_messages#11|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery|_drain#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.sqs-lambda-event-source-mapping-basic|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__sqs.sqs-to-lambda-event-source-mapping|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll|run#6|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive|run#18|AssertionError|assert delivery_attempts >= 2, (
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs|_collect#10|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions|_collect#10|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__lambda__sqs|GetPolicy|ResourceNotFoundException
Issue-Commit: 5b7ade256eac0e0bdb0fb9604620cb62d25f829f

# events/lambda/sqs: 6 of the 7 causes are still open — a Lambda function never RUNS its deployment package, so neither an EventBridge rule's invoked function nor an SQS event source mapping has any observable effect, durable-execution status events are never published, and a CloudFormation-created function is invisible to Lambda

> _Split out of `issues/things-that-now-work/20261029T182228Z-events-lambda-sqs-lambda-targets-and-sqs-esms-never-fire-seven-remaining-causes.md`, whose cause 5 is fixed (and whose wording claim about it was the opposite of the recording — see below) and whose cause 3 is fixed as far as AUTHORIZATION and DEAD-LETTERING go: an EventBridge rule now decides a Lambda function target's delivery against that function's own resource-based policy and dead-letters the refusal. This report carries the replay keys that do NOT pass, re-filed here so they are not recorded as working. Two of them have MOVED to a later step since that fix and are re-filed at the step they now fail at (`eventbridge-lambda-target-missing-permission-dlq`, `eventbridge-rule-dlq-on-failed-lambda-invoke`). Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
Invoke

## Description

**One capability blocks five of the six.** Causes 2, 4, 6, 7 and the positive half of cause 3
all wait on a Lambda function RUNNING its deployment package. Every one of their checks asserts
on a queue, a table or an event that the function under test produces, and
`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` states in its own
header that "running the deployment package is NOT implemented: this emulator has no in-process
runtime for a Lambda zip archive or container image", so `RequestResponse` answers HTTP 501 and
a queued asynchronous `Event` settles into the outcome that records it never ran. The WIRING
around it is already real and is not what is missing: `state::sqs_poller` polls an enabled SQS
event source mapping through the queue's own delivery bookkeeping, batches to `BatchSize`,
applies `FilterCriteria`, calls `Invoke` over the in-process callback, deletes only what an
invocation accepted and publishes the event counts; EventBridge invokes a function target
asynchronously and now authorizes it against the function's resource policy. That engine is the
subject of `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`
and `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`;
`retrieve_doc(doc_id="implementing-code-execution")` is the SOP for building it.

Cause 2 of 8 (kept numbered as first filed): an EventBridge rule's SQS target never receives
anything in the three remaining traces.
AWS: a matching event reaches every SQS target of every matching rule exactly once.
Emulator: "AssertionError: event never delivered to <queue url> after 8 attempts of 6.0s each"
and "event never delivered to: [<queue url>] after 15 attempts" — the queue URL is a per-run
value; the requirement is one message per matched rule. All three remaining traces are
lambda-to-SQS CHAINS: the queue they read is one the invoked FUNCTION writes to, so they wait on
the execution engine rather than on the rule dispatcher, whose own queue-target delivery and
multi-rule fan-out are exercised by
`services/service-com-amazonaws-eventbridge-awsevents/tests/all/target_delivery.rs` and
`.../rule_fan_out_and_target_repointing.rs`.
Traces: events__lambda__sqs.eventbridge-event-envelope-format-to-lambda,
events__lambda__sqs.eventbridge-lambda-sourcearn-condition-arn-resolution,
events__lambda__sqs.eventbridge-multiple-rules-single-event-fanout

Cause 3 of 8, POSITIVE HALF ONLY (the authorization and dead-letter half is fixed): an
EventBridge rule's Lambda target is invoked, but the invocation does nothing, and a scheduled
rule never fires at all.
AWS: a matching `PutEvents`, or a `ScheduleExpression` falling due, invokes each Lambda target
once with the envelope (or the `InputTransformer` rendering); `RemoveTargets` stops it.
Emulator: the invocation is accepted and queued and the function never runs, so every check that
reads the function's effect ends "Check timed out after 60.0s"; and a rule carrying a
`ScheduleExpression` has no timer behind it, so it fires nothing even for a target whose effect
IS observable — that half is independent of the execution engine.
Fix: the Lambda execution engine, plus the scheduled-rule timer in
`service-com-amazonaws-eventbridge-awsevents` (a rule with a `ScheduleExpression` must deliver
to its targets when the expression falls due, driven the way
`service-com-amazonaws-lambda-awsgirapiservice/src/state/sqs_poller.rs::drive_sqs_pollers`
drives a poller: a background task claimed once per store, never a sleep inside a request).
Traces: events__lambda__sqs.cli-eventbridge-remove-targets-stops-delivery-negative,
events__lambda__sqs.cli-lambda-add-permission-for-eventbridge-invoke,
events__lambda__sqs.cli-put-rule-put-targets-lambda-sqs-fanout,
events__lambda__sqs.eventbridge-fanout-lambda-and-sqs,
events__lambda__sqs.eventbridge-lambda-target-missing-permission-dlq,
events__lambda__sqs.eventbridge-lambda-target-without-resource-policy-no-invoke,
events__lambda__sqs.eventbridge-rule-dlq-on-failed-lambda-invoke,
events__lambda__sqs.eventbridge-rule-input-transformer-reshapes-lambda-payload,
events__lambda__sqs.eventbridge-rule-sqs-lambda-nested-detail-survives-to-dynamodb,
events__lambda__sqs.eventbridge-schedule-expression-triggers-lambda,
events__lambda__sqs.eventbridge-to-lambda-to-sqs-chain,
events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions

Cause 4 of 8: an SQS event source mapping polls and invokes, but the invocation runs nothing, so
none of `BatchSize`, `Enabled`, partial-batch reporting, DLQ-on-repeated-failure or FIFO
`MessageGroupId` ordering has any observable effect.
AWS: an enabled mapping polls the queue and invokes the function with up to `BatchSize` records;
a disabled mapping stops polling; a `ReportBatchItemFailures` response retries only the listed
`itemIdentifiers`; a FIFO source preserves per-`MessageGroupId` order.
Emulator: the function never runs, so the batch is never deleted and no response document is
ever produced to read a partial failure out of; 10 checks end "Check timed out after 60.0s".
Fix: the execution engine. The poller itself, including the partial-batch handling and the FIFO
group ordering, is in `state::sqs_poller` and is driven; what it has no answer to read is the
invocation's.
Traces: events__lambda__sqs.cli-create-event-source-mapping-sqs-lambda,
events__lambda__sqs.cli-delete-event-source-mapping-stops-polling,
events__lambda__sqs.lambda-esm-dlq-on-repeated-failure,
events__lambda__sqs.lambda-sqs-partial-batch-failure-reporting,
events__lambda__sqs.sqs-esm-batch-size-controls-invocation-count,
events__lambda__sqs.sqs-esm-disabled-mapping-stops-delivery,
events__lambda__sqs.sqs-esm-partial-batch-failure-retries-only-failed,
events__lambda__sqs.sqs-fifo-esm-preserves-message-group-order,
events__lambda__sqs.sqs-lambda-event-source-mapping-basic,
events__lambda__sqs.sqs-to-lambda-event-source-mapping

Cause 5 of 8 is FIXED and this report carries only its remaining check. The gate exists
(`create_event_source_mapping.rs` calls `state::unreadable_event_source`, refusing with the
modelled 400 `InvalidParameterValueException`) and its wording is the recording's: AWS answers
"The function execution role does not have permissions to call ReceiveMessage on SQS", which is
what `state::event_source_authorization::EventSourceAccess::message` builds. The parent report
asked for "The provided execution role …"; three later reports of the same checks
(`20261030T110325Z-lambda-s3-sqs-createeventsourcemapping-s-unauthorized-execution-role.md`,
`20261030T121631Z-events-lambda-sqs-createeventsourcemapping-s-under-privileged-execution.md`,
`20261101T043227Z-events-lambda-sns-sqs-createeventsourcemapping-s-missing-sqs.md`) carry the
replay assertion verbatim and it is the opposite, so the current wording stands. Its one
remaining check does not assert on the refusal at all: it asserts that such a mapping does not
DRAIN the queue and never invokes the function, which waits on the execution engine like cause 4.
Traces: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll

Cause 6 of 8: a queue's redrive policy is reported as moving a message to the DLQ after zero
delivery attempts.
AWS: with a `RedrivePolicy` of `maxReceiveCount=N`, a message moves to the dead-letter queue
only after it has been received N times; with N=2 it must survive at least the first receive.
Emulator: "AssertionError: message reached the DLQ after 0 delivery attempt(s);
maxReceiveCount=2 requires it to survive at least the first" — `maxReceiveCount` is a value from
the request; the rule, move only once the count is reached, is the requirement. The "0 delivery
attempt(s)" is counted by the scenario's own function, which never runs (cause 4); the SQS store
already moves a message only once its per-queue receive count has reached `maxReceiveCount`, so
this cause has to be re-checked once the mapping's invocations run — it may be a symptom of
cause 4 rather than a cause of its own.
Traces: events__lambda__sqs.sqs-esm-dlq-redrive-after-max-receive

Cause 7 of 8: Lambda durable-execution status-change events are never published to EventBridge.
AWS: a durable execution publishes a status-change event onto the DEFAULT bus with source
`aws.lambda`, whose `detail` carries at least `durableExecutionArn` and `status` (`RUNNING`,
then one of `SUCCEEDED`/`FAILED`/`TIMED_OUT`/`STOPPED`) and, for a terminal one, an
`endTimestamp`; a rule scoped to one status receives only that status, and a `TIMED_OUT` event's
`endTimestamp` is at least `DurableConfig.ExecutionTimeout` after the start.
Emulator: the rule's SQS target stays empty for every status; both checks end "Check timed out
after 60.0s".
Fix: the durable-execution slice of the Lambda state must publish its status changes onto the
default bus. `state::DurableExecutionRecord::materialize` already applies the `TIMED_OUT`
transition the clock owes and `stop` the `STOPPED` one, so what is missing is the publication and
a driver that applies a due transition without waiting for a caller; `SUCCEEDED` and `FAILED`
additionally need the execution engine. The same cause is filed as
`20261027T191349Z-events-lambda-sqs-lambda-durable-execution-status-change-events-are-never.md`
(the `STOPPED` check) and as causes 4-6 of
`20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md`; those reports carry the
recorded detail member names.
Traces: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs,
events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions

Cause 8 of 8: a Lambda function created by CloudFormation is invisible to the Lambda data plane.
AWS: after the stack reaches `CREATE_COMPLETE`, `GetPolicy`/`GetFunction`/`Invoke` on the
stack-generated physical id succeed.
Emulator: `GetPolicy` answers HTTP 404 `ResourceNotFoundException` "Function not found: <function
ARN>" — "Function not found: " is fixed text and the ARN is the stack's generated physical id.
`state::template::provision` computes the physical id and the `Arn` attribute for
`AWS::Lambda::Function`, but `state::provisioner::provisioner_for` has no arm for that type, so
nothing is ever created in the Lambda crate's own store.
Fix: the CloudFormation `AWS::Lambda::Function` provisioner must call that service's own
`CreateFunction` and record what it answered, the way the arms beside it do. NOTE for the fixer:
`services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs` is 28,667
lines, over the 5,000-line ceiling the merge gate enforces, so it has to be split into a module
directory as its own change before the arm can be added. The same cause is filed for other
services as
`20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md`.
Traces: events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target

## Evidence
Cause 2: "AssertionError: event never delivered to https://sqs.<region>.amazonaws.com/<account>/<queue> after 8 attempts of 6.0s each"; "event never delivered to: [...] after 15 attempts".
Causes 3, 4 and 7: 24 checks end "Error: Check timed out after 60.0s".
Cause 6: "AssertionError: message reached the DLQ after 0 delivery attempt(s); maxReceiveCount=2 requires it to survive at least the first".
Cause 8: "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPolicy operation: Function not found: arn:aws:lambda:<region>:<account>:function:<stack>-ForwarderFunction-<suffix>".

## Steps to Reproduce
(1) CreateFunction with a handler that forwards to a queue, PutRule with that function as a target plus AddPermission for events.amazonaws.com, PutEvents a matching entry, read the queue — nothing arrives, because the handler never runs. (2) PutRule with a ScheduleExpression and any target — it never fires. (3) CreateEventSourceMapping on a standard and a FIFO queue, SendMessage — the mapping polls, the invocation is refused and the messages come back. (4) SetQueueAttributes with RedrivePolicy maxReceiveCount=2, SendMessage, let a mapping poll it. (5) Invoke a DurableConfig function and poll the SQS target of a default-bus rule matching aws.lambda status changes — empty for RUNNING, TIMED_OUT and STOPPED. (6) CreateStack with AWS::Lambda::Function, then GetPolicy on the physical id — 404.
