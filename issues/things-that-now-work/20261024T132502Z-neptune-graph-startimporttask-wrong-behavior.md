Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Every import task ends FAILED instead of progressing to SUCCEEDED, and importTaskDetails is never reported

_Filed: 2026-10-24T13:25:02Z_
_Operation: StartImportTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartImportTask (and CreateGraphUsingImportTask, which shares the same record and settle path) persists a real import-task record, but the task's fate is hardcoded to failure: the record is stored with status INITIALIZING and an owed transition to ImportTaskStatus::Failed carrying crate::state::IMPORT_NOT_RUN_REASON, so after ~500ms every accepted task reports FAILED forever.

Expected (AWS): an import accepted against an AVAILABLE graph with a well-formed S3 `source`, `roleArn` and `format` reports INITIALIZING, then a loading stage (ANALYZING_DATA / IMPORTING / REPROVISIONING), and finally the terminal SUCCEEDED, with `importTaskDetails` carrying status, startTime, timeElapsedSeconds, a rising progressPercentage, errorCount, statementCount and dictionaryEntryCount, and no emulator-authored statusReason.

Actual: GetImportTask reports INITIALIZING briefly and then FAILED with an emulator-authored statusReason; the enum never produces ANALYZING_DATA / IMPORTING / SUCCEEDED, and `importTaskDetails` is hardcoded to None in the GetImportTaskOutput projection, so a client that polls an import to completion — the normal use of this API, and the only way to use CreateGraphUsingImportTask at all — always takes the failure branch on a request AWS would have completed successfully. The graph created by CreateGraphUsingImportTask does come up AVAILABLE, so the graph/task pair is internally inconsistent: the graph claims to be ready while its only load reports it never ran.

This is the import twin of the already-filed StartExportTask issue ("Every export task ends FAILED instead of progressing to SUCCEEDED"), which covers the export path only.

## Evidence

Spec: the model's ImportTaskStatus enum declares ANALYZING_DATA | CANCELLED | CANCELLING | DELETED | EXPORTING | FAILED | IMPORTING | INITIALIZING | REPROVISIONING | ROLLING_BACK | SUCCEEDED, and GetImportTask's output declares `importTaskDetails` (ImportTaskDetails, with required status/startTime/timeElapsedSeconds/progressPercentage/errorCount/statementCount/dictionaryEntryCount) and `attemptNumber` — i.e. a successful, progress-reporting load is the modelled happy path and FAILED is for an import that genuinely could not run.
Handler/state: src/ops/start_import_task.rs lines ~306-321 store the task as Initializing and call begin_transition(Initializing -> Failed, Some(IMPORT_NOT_RUN_REASON)); src/ops/create_graph_using_import_task.rs lines ~424-440 do the same; src/state.rs IMPORT_NOT_RUN_REASON (line ~1465) states no load is ever performed, and the GetImportTaskOutput projection in src/state.rs line ~2060 hardcodes `import_task_details: None`.
Test coverage: tests/all/import_task_lifecycle.rs (lines ~381-387) and tests/all/create_graph_using_import_task.rs (lines ~581, 663) assert the divergent FAILED outcome, so the divergence is locked in by tests; no test asserts an import reaching SUCCEEDED or any importTaskDetails being reported.
Backlog: the open neptune-graph issues cover the export-task terminal status, the task 404 paths, roleArn authorization, region scoping and kmsKeyIdentifier/buildNumber — none covers the import-task terminal status or importTaskDetails.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=import-probe, provisionedMemory=16) and poll GetGraph until status is AVAILABLE.
3. StartImportTask(graphIdentifier=<id>, source='s3://my-bucket/data/', format='CSV', roleArn='arn:aws:iam::123456789012:role/import-role') -> 201 with status INITIALIZING.
4. Poll GetImportTask(taskIdentifier=<taskId>) until the status stops changing.
5. Observe status FAILED with an emulator-authored statusReason and no `importTaskDetails` member at all; expected SUCCEEDED (after a loading stage) with importTaskDetails reporting progressPercentage 100 and the statement/dictionary/error counts.
6. Repeat with CreateGraphUsingImportTask(graphName=..., source=..., roleArn=...): the graph reaches AVAILABLE while GetImportTask for the returned taskId reports FAILED.
