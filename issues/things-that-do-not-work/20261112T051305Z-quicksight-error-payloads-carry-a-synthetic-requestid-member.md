Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-data-source-lifecycle-create-update-delete
Issue-Step: task#9
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-data-source-lifecycle-create-update-delete quicksight-folder-lifecycle-and-duplicate-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|DescribeDataSource|4xx->4xx|$.RequestId
Issue-Key: quicksight|CreateFolder|4xx->4xx|$.RequestId
Issue-Commit: 084fe38e94c6ae3d12f6c6daafcb56ce742ec067

# QuickSight error payloads carry a synthetic RequestId member AWS omits

> _Note: this issue was identified against the code base at commit `084fe38e94c6ae3d12f6c6daafcb56ce742ec067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataSource

## Description
The emulator's QuickSight error serializer unconditionally populates the `RequestId` member of the error payload with a synthetic, counter-derived UUID. AWS QuickSight returns the request id only in the `x-amzn-RequestId` response header for these error classes; the JSON error body contains just the error's own members (`Message`, and `ResourceType` where modeled), with `RequestId` absent. Every replayed QuickSight error body in this corpus asserts `RequestId` absent, so the extra member is a shape divergence of the error payload itself, not a per-run value difference: the fixtures do not disagree about the id's VALUE, they disagree about the member existing at all. Both causes below are the same serializer behavior on two different error classes and two different operations; they diverge ONLY in that member — status, error type header, error code and message all match the recording in both.

Cause 1 of 2: DescribeDataSource on a deleted data source — 404 ResourceNotFoundException.
AWS: Smithy: DescribeDataSource errors include ResourceNotFoundException (errorKind client, httpError 404); shape ResourceNotFoundException members = Message, ResourceType, RequestId (all optional). Recorded behavior: the 404 body carries the ResourceNotFoundException message and NO RequestId member; the request id travels in the response header. The message template for this class is fixed text plus values from the request/state — "DataSource <arn> is not found", where the fixed text is "DataSource " and " is not found" and the variable part is the data source's ARN built from partition, region, account id and data source id (the recording's `arn:aws:quicksight:us-west-2:000000000000:datasource/sim2real-probe-qs-ds-c37f2ca1` is one run's filling of that template, not the requirement). That message matched on replay; only the extra member diverged.
Emulator: returns 404 with the correct message, but the body additionally contains `"RequestId": "00000000-0000-4000-8000-000000000006"` where the recording has no such member.

Cause 2 of 2: CreateFolder with an id that already exists — 409 ResourceExistsException.
AWS: Smithy: CreateFolder errors include ResourceExistsException (errorKind client, httpError 409); shape ResourceExistsException members = Message, ResourceType, RequestId (all optional). Recorded behavior: the 409 body carries the duplicate-folder message and NO RequestId member.
Emulator: returns 409 with the expected status and message plus `"RequestId": "00000000-0000-4000-8000-000000000004"`, a member absent from the recording.

Why: a client or SDK test that round-trips or snapshots a QuickSight error payload sees a member AWS never sends, and the value it sees is a deterministic placeholder rather than a real request id — so correlating a failure with a service-side request id by reading the body succeeds against the emulator and silently yields nothing against AWS. Strict error-body comparisons (contract tests, recorded-fixture assertions) fail on the emulator for errors that are otherwise exactly right.

Fix: the QuickSight error-response serializer (the shared error-payload builder that fills modeled exception members before writing the body). It should leave `RequestId` unset in the serialized error body and surface the request id through the `x-amzn-RequestId` response header only, rather than defaulting the modeled member from the request-id allocator. The change is in serialization, not in the handlers: both handlers already produce the right status, error type and message.

Traces: quicksight-data-source-lifecycle-create-update-delete#task#9 (DescribeDataSource, 404); quicksight-folder-lifecycle-and-duplicate-rejected#task#5 (CreateFolder, 409).

Related open issues that do NOT cover this: `20261109T121538Z-quicksight-accepts-rejected-creates-and-keeps-deleted-resources.md` owns the clusters where QuickSight answers 200 instead of the modeled 400/404 (those clusters also show the stray RequestId, but their replay key is bound to the status divergence); `20261031T224711Z-quicksight-data-source-resourcenotfoundexception-message-is-invented-wording.md` owns the data-source not-found MESSAGE wording. Neither owns the two clusters filed here, whose status and message already match and whose only divergence is the extra error-body member.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 — fixture `quicksight-data-source-lifecycle-create-update-delete`, step `task#9`, DescribeDataSource, verdict mismatch, expected_status 404 / actual_status 404:
  $.RequestId: type_mismatch (expected None, got '00000000-0000-4000-8000-000000000006')
No other key diverged: status, x-amzn-errortype and $.Message all matched the recorded ResourceNotFoundException.

Cause 2 — fixture `quicksight-folder-lifecycle-and-duplicate-rejected`, step `task#5`, CreateFolder, verdict mismatch, expected_status 409 / actual_status 409:
  $.RequestId: type_mismatch (expected None, got '00000000-0000-4000-8000-000000000004')
No other key diverged.

Expected (AWS, per recording + Smithy): error body = {"Message": "<class template filled from request/state>"} (plus ResourceType where the service sets it), with the request id only in the x-amzn-RequestId header.
Actual (emulator): the same body plus a `RequestId` member holding a synthetic sequential UUID.

Model references: quicksight-20180401 — DescribeDataSource -> ResourceNotFoundException (404); CreateFolder -> ResourceExistsException (409); both shapes model RequestId as an OPTIONAL member, so emitting it is not required by the model and the recordings show AWS leaving it out.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture `quicksight-data-source-lifecycle-create-update-delete` through step `task#9` — create a data source, delete it, then call DescribeDataSource on the deleted DataSourceId. The emulator answers 404 with the right ResourceNotFoundException message; diff the JSON body against the recording and observe the extra top-level `RequestId` member (recording has none).
3. Cause 2: replay fixture `quicksight-folder-lifecycle-and-duplicate-rejected` through step `task#5` — CreateFolder with a FolderId that already exists. The emulator answers 409 ResourceExistsException with the right message plus the extra top-level `RequestId` member.
4. In both cases compare only the body members: the divergence is the presence of `RequestId`, independent of its value, so it reproduces on every run despite the id being deterministic.
