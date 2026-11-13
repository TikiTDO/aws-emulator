Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.snapshot-of-ready-application-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2.snapshot-of-ready-application-rejected|run#27|AssertionError|assert delete_err["Code"] == "ResourceNotFoundException", (
Issue-Commit: 08254eb51951b8f15d0d2d2a378e30e9ecb39ad1

# DeleteApplicationSnapshot is an unimplemented scaffold stub: answers 501 NotImplemented instead of the modeled snapshot refusals

> _Note: this issue was identified against the code base at commit `08254eb51951b8f15d0d2d2a378e30e9ecb39ad1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplicationSnapshot

## Description
1 cause.

Cause 1 of 1: `DeleteApplicationSnapshot` is not implemented in the kinesisanalyticsv2 emulator — every call to it answers HTTP 501 with error code `NotImplemented`, so the snapshot lifecycle cannot be exercised past a create/describe.

AWS: Smithy `DeleteApplicationSnapshot` input is `ApplicationName` (ApplicationName, @required, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$), `SnapshotName` (SnapshotName, @required, @length 1..256, @pattern ^[a-zA-Z0-9_.-]+$) and `SnapshotCreationTimestamp` (Timestamp, @required); its output shape `DeleteApplicationSnapshotResponse` is empty (HTTP 200, no members). The operation declares the client errors `ResourceNotFoundException` ("Specified application can't be found.", HTTP 400), `InvalidArgumentException` (HTTP 400), `InvalidRequestException` (HTTP 400), `ResourceInUseException` (HTTP 400), `UnsupportedOperationException` (HTTP 400) and `ConcurrentModificationException` (modeled httpError 409). Deleting a snapshot name that does not exist under an application that DOES exist is refused with error code `ResourceNotFoundException` and the message `Snapshot '<snapshot-name>' does not exist for application <application-name>.` — the fixed text is everything outside the two substitutions; `<snapshot-name>` is the request's `SnapshotName` and `<application-name>` is the request's `ApplicationName` (this is the same message AWS returns from `DescribeApplicationSnapshot` for the same condition, which this emulator already produces correctly, so the two operations must agree). Recorded behavior only for the exact message template; the error class is what the model declares.

Emulator: the handler body is still the generated scaffold — `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteApplicationSnapshot"))` — so the call answers HTTP 501 with code `NotImplemented` and message `operation DeleteApplicationSnapshot is scaffolded but not implemented in this emulator`, for every input, valid or not. In the replayed check the call `DeleteApplicationSnapshot(ApplicationName=<app>, SnapshotName=<snapshot never minted>, SnapshotCreationTimestamp=<ts>)` returned code 'NotImplemented' where AWS returns 'ResourceNotFoundException'.

Why: a client cannot delete a snapshot at all against this emulator, and cannot distinguish "the snapshot is not there" (retry-free, idempotent-cleanup path) from a server fault: 501/NotImplemented is not a modeled error of the API, so SDK error handling keyed on `ResourceNotFoundException`/`InvalidArgumentException` never matches and generic retry logic treats the refusal as a transient server error. Snapshot cleanup loops (delete-if-exists) therefore either abort or spin, and no test that drives the full snapshot lifecycle (create → list → describe → delete) can pass.

Fix: implement the handler in services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/delete_application_snapshot.rs (the only file for this operation) against the service's snapshot state: look the application up (missing application → `InvalidArgumentException`, as the sibling snapshot operations do), then the snapshot by name (missing → `ResourceNotFoundException` with the message template above), then verify `SnapshotCreationTimestamp` matches the stored snapshot's creation timestamp before removing the record and answering an empty 200; delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line at the top of that file once done.

Traces: kinesisanalyticsv2.snapshot-of-ready-application-rejected (the check's earlier assertions on CreateApplicationSnapshot's `InvalidRequestException` for a READY application and on DescribeApplicationSnapshot's `ResourceNotFoundException` passed; it fails on the first DeleteApplicationSnapshot call).

Relation to open issues: no open issue names `DeleteApplicationSnapshot` — the filed stub issues cover AddApplicationCloudWatchLoggingOption, CreateApplicationPresignedUrl, DescribeApplicationVersion and ListTagsForResource, and 20261103T022644Z-kinesisanalyticsv2-studio-flink-runtime-rules-unenforced-createapplication-fails-open.md / 20261103T054447Z-kinesisanalyticsv2-refusal-responses-diverge-wrong-error-class-wrong.md concern CreateApplication/UpdateApplication/DeleteApplication refusal classes, not this operation.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against the emulator built from this checkout.

Failing check: kinesisanalyticsv2.snapshot-of-ready-application-rejected [mismatch]
  AssertionError: deleting the snapshot that was never minted reported 'NotImplemented', expected 'ResourceNotFoundException'
  (check_kinesisanalyticsv2_snapshot_of_ready_application_rejected.py line 164)

Expected (AWS): HTTP 400, error code `ResourceNotFoundException`, message "Snapshot '<SnapshotName>' does not exist for application <ApplicationName>." — fixed text apart from the two values, which come from the request.
Actual (emulator): HTTP 501, error code `NotImplemented`, message "operation DeleteApplicationSnapshot is scaffolded but not implemented in this emulator".

Source: services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/delete_application_snapshot.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and line 166 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteApplicationSnapshot"))`).

Smithy citations (services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json): DeleteApplicationSnapshot input ApplicationName @required length 1..128 pattern ^[a-zA-Z0-9_.-]+$, SnapshotName @required length 1..256 pattern ^[a-zA-Z0-9_.-]+$, SnapshotCreationTimestamp -> Timestamp @required; empty output; errors ResourceNotFoundException/InvalidArgumentException/InvalidRequestException/ResourceInUseException/UnsupportedOperationException (HTTP 400) and ConcurrentModificationException (HTTP 409).

Note: the emulator already serves the correct `ResourceNotFoundException` + "Snapshot '...' does not exist for application ..." pair from DescribeApplicationSnapshot in the same check, so the expected wording is observable in-tree.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the behavioral check kinesisanalyticsv2.snapshot-of-ready-application-rejected, or by hand:
   a. iam.CreateRole with a trust policy for kinesisanalytics.amazonaws.com.
   b. kinesisanalyticsv2.CreateApplication(RuntimeEnvironment='FLINK-1_15', ServiceExecutionRole=<that role ARN>, ApplicationConfiguration={'ApplicationSnapshotConfiguration': {'SnapshotsEnabled': True}}) — the application is READY with no snapshots.
   c. kinesisanalyticsv2.DeleteApplicationSnapshot(ApplicationName=<app>, SnapshotName='sim2real-behav-snap-1', SnapshotCreationTimestamp=1700000000) for a snapshot that was never created.
3. Observe HTTP 501 with error code 'NotImplemented' instead of HTTP 400 with 'ResourceNotFoundException' and message "Snapshot 'sim2real-behav-snap-1' does not exist for application <app>." (the quoted names are this run's values filling the template).
4. For comparison, kinesisanalyticsv2.DescribeApplicationSnapshot with the same names already answers the expected code and message.
