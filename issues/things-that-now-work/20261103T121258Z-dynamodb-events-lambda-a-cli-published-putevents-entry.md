Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow
Issue-Key: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow|_poll_for_item#28|Timeout|return None
Issue-Commit: 18c5740d8e571647dfc03b529698248398b6edec

# dynamodb__events__lambda: a CLI-published PutEvents entry matching a rule never reaches the rule's Lambda target, so the DynamoDB item is never written

> _Note: this issue was identified against the code base at commit `18c5740d8e571647dfc03b529698248398b6edec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause (1 cause in this issue).

Cause 1 of 1: an event published with `aws events put-events` matches an enabled rule whose Lambda target should write the event detail's id/value into DynamoDB, but the target is never invoked, so GetItem on that key never returns an item and the check exhausts its budget.
Failing check: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow

AWS: PutEvents accepts the entry and answers 200 with FailedEntryCount 0 and one PutEventsResultEntry carrying an EventId; EventBridge then matches the entry against the enabled rules of the bus and asynchronously invokes every target the target's own policy admits — here a Lambda function whose resource policy allows lambda:InvokeFunction to principal events.amazonaws.com with SourceArn = the rule ARN. The function runs once per matching entry with the event envelope ({source, detail-type, detail, ...}) and its dynamodb:PutItem makes a subsequent GetItem on the detail's id return the item with the sent value. Smithy (eventbridge, services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json): PutEvents.Entries @required @length(min 1, max 10); output FailedEntryCount (Integer, default 0) + Entries (PutEventsResultEntryList); its only modeled error is InternalException (500, server). PutTargets.Rule and PutTargets.Targets are @required with output FailedEntryCount/FailedEntries and client errors ConcurrentModificationException / LimitExceededException / ManagedRuleException / ResourceNotFoundException (400). Delivery to a target is therefore NOT reported in any response — acceptance plus the modeled empty FailedEntries set is the only acknowledgement a client ever gets, so a silently undelivered event is unobservable through the API. The invocation itself is recorded behavior only.
Emulator: every control-plane call of the wiring is accepted — create_table, create_role/put_role_policy, create_function (Publish=true, waiter reaches Active), `aws events put-rule`, `aws lambda add-permission` for events.amazonaws.com scoped to the rule ARN, `aws events put-targets` with the function ARN, and `aws events put-events` with a matching Source — yet the target function is never run: GetItem for the detail's id answers with no Item on every poll. The check re-publishes the same entry and polls GetItem every 2s for up to 150s and the replay ends `Error: Check timed out after 60.0s`, i.e. the rule->Lambda delivery never happens at all rather than happening late. Nothing in any response signals the failure: PutEvents reports no failed entry and PutTargets reports no failed target.
Why: a client that wires the most common serverless pipeline (publish an event, let a rule invoke a function, the function persists the record) is told by every API that the wiring is complete, and then nothing downstream ever happens and there is no error to act on. Every retry and every timeout budget built on "the event was accepted, so the target will run" is wrong against this emulator, and an application cannot distinguish "not delivered yet" from "never delivered".
Fix: the EventBridge rule-matching and target-dispatch path of the events crate — pattern match of a PutEvents entry against the bus's enabled rules, then authorization of the Lambda target against the function's resource policy (principal events.amazonaws.com, SourceArn = rule ARN) and the actual Lambda Invoke it must drive — together with the state that records a rule's targets. The CLI is not implicated: the publish path is the same for an SDK caller.
Related open issues (neither carries this check as a replay key the suite recognised for this run, so this finding is filed rather than skipped): 20261102T091831Z-dynamodb-events-lambda-a-cli-issued-put-events.md lists this same check id in its Issue-Trace-Ids but under a different replay key (`_poll_for_item#27|Timeout|time.sleep(interval_s)`), while this run failed on the harness-level 60s budget — a fixer should close both together; 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md owns the sibling checks of this group; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md names the Invoke side this delivery depends on.
Traces: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow.

## Evidence
Expected (AWS): `aws events put-events --entries '[{"Source":<unique source>,"DetailType":"sim2real-pipeline","Detail":"{\"id\":<item id>,\"value\":<value>}"}]'` returns FailedEntryCount 0 with one EventId; the matching rule invokes the Lambda target once; GetItem{TableName=<table>, Key={id:{S:<item id>}}} then returns Item with value.S == the published value.
Actual (emulator): the same sequence is accepted end to end (PutRule, AddPermission, PutTargets, PutEvents all succeed and report no failed entries), but GetItem answers with no `Item` on every poll — the target function is never invoked and no item is ever written. The replayed check ends with `Error: Check timed out after 60.0s` after re-publishing the entry and polling GetItem every 2s.
The absent item, not the wall-clock number, is the divergence: the 60s figure is this run's harness budget and the table/function/rule names and item ids are per-run values.

## Steps to Reproduce
1. Create a DynamoDB table with key `id` (S), PAY_PER_REQUEST, and wait for ACTIVE.
2. Create an execution role trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole and an inline statement allowing dynamodb:PutItem on the table ARN.
3. Create a python3.12 function whose handler does `ddb.put_item(TableName=os.environ['TABLE_NAME'], Item={'id': {'S': event['detail']['id']}, 'value': {'S': event['detail']['value']}})`; wait for Active.
4. `aws events put-rule --name <rule> --event-pattern '{"source":["<unique source>"]}'`.
5. `aws lambda add-permission --function-name <fn> --statement-id AllowEventBridgeInvoke --action lambda:InvokeFunction --principal events.amazonaws.com --source-arn arn:aws:events:<region>:<account>:rule/<rule>`.
6. `aws events put-targets --rule <rule> --targets '[{"Id":"1","Arn":"<fn arn>"}]'`.
7. `aws events put-events --entries '[{"Source":"<unique source>","DetailType":"sim2real-pipeline","Detail":"{\"id\":\"<item id>\",\"value\":\"pipeline-value-42\"}"}]'` — observe FailedEntryCount 0.
8. Poll `GetItem` for `{id: {S: <item id>}}`: on AWS the item appears with the sent value; on the emulator it never appears, and re-publishing the entry does not change that.
