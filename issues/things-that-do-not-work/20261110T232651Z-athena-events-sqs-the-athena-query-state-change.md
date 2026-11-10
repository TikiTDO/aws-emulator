Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed
Issue-Key: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed|_collect_event#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# athena__events__sqs: the Athena query-state-change flow never reaches a verdict — no "Athena Query State Change" event is observable within the check's budget

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
One net-new root cause of service `athena__events__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — a query driven to SUCCEEDED and a query driven to FAILED produce no EventBridge-delivered state-change event, so the flow never settles.
AWS: recorded behavior only (Athena's EventBridge integration is not part of the Smithy model). Athena publishes an `Athena Query State Change` event on the default bus with `source` = `aws.athena`, `detail-type` = `Athena Query State Change`, and a `detail` carrying `currentState`, `previousState`, `queryExecutionId`, `workgroupName`, `statementType` and `sequenceNumber`; `detail.athenaError` (with `errorCategory`/`errorType`/`errorMessage`) is present ONLY for a query whose currentState is FAILED, and is absent for SUCCEEDED. A rule matching that pattern delivers the envelope to its SQS target within seconds of the query settling.
Emulator: the check exhausts its whole budget without the asserted observation ever appearing — `Error: Check timed out after 60.0s`. Nothing in the leg completed: no delivered envelope was seen for either the succeeded or the failed query.
Why: a client that keys off `Athena Query State Change` to react to query completion (or to read `athenaError` on failure) never receives anything, and cannot distinguish a still-running query from a settled one.
Fix: the Athena query-execution state machine's event-emission path (the component that must publish to EventBridge when a query execution transitions), plus its EventBridge ingestion. Related fixed issue 20261108T041854Z-athena-query-failures-are-misclassified-athenaerror-errortype-absent.md covers the AthenaError CLASSIFICATION on the Athena API itself, not the emission of the EventBridge state-change event asserted here.
Traces: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed

## Evidence
Check athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed
Expected: a rule on {"source":["aws.athena"],"detail-type":["Athena Query State Change"]} delivers one envelope per transition to its SQS target; the FAILED query's detail carries athenaError, the SUCCEEDED query's does not.
Actual: Error: Check timed out after 60.0s — no envelope was ever observed, the check produced no verdict.

## Steps to Reproduce
1. Boot the emulator and replay athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed.
2. Create an SQS queue with a policy allowing events.amazonaws.com SendMessage, and an EventBridge rule on {"source":["aws.athena"],"detail-type":["Athena Query State Change"]} targeting it.
3. StartQueryExecution with a valid statement and poll GetQueryExecution to SUCCEEDED; then StartQueryExecution with a statement that fails and poll to FAILED.
4. ReceiveMessage on the queue: expect two envelopes, the FAILED one carrying detail.athenaError and the SUCCEEDED one not carrying it. The emulator delivers nothing.
