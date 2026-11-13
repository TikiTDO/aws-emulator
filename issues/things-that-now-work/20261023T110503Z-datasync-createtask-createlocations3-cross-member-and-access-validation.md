Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datasync.create-location-s3-unreadable-bucket-rejected datasync.create-task-rejects-same-source-and-destination-location datasync.create-task-rejects-transfer-all-with-overwrite-never datasync.execution-denied-when-role-loses-source-bucket-access datasync.overwrite-mode-never-preserves-existing-destination-objects datasync.s3-to-s3-agentless-transfer-copies-objects-and-counters
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync.create-location-s3-unreadable-bucket-rejected|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code to read")
Issue-Key: datasync.create-task-rejects-same-source-and-destination-location|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code to read")
Issue-Key: datasync.create-task-rejects-transfer-all-with-overwrite-never|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code to read")
Issue-Key: datasync.execution-denied-when-role-loses-source-bucket-access|run#19|AssertionError|assert denied_exec["Status"] == "ERROR", (
Issue-Key: datasync.overwrite-mode-never-preserves-existing-destination-objects|run#13|AssertionError|assert never_exec["Status"] == "ERROR", (
Issue-Key: datasync.s3-to-s3-agentless-transfer-copies-objects-and-counters|run#21|AssertionError|assert execution["EstimatedFilesToTransfer"] == len(_OBJECTS), (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# DataSync: CreateTask/CreateLocationS3 cross-member and access validation missing, and task executions are a fixed stub that transfers nothing (6 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTask

## Description
Six distinct root causes in the DataSync emulator, all grounded in failing behavioral checks. The first three are control-plane validation that is absent; the last three are the task-execution state machine, which returns one fixed, all-SUCCESS result with zero counters and never moves any object bytes. Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json (CreateTask, CreateLocationS3, DescribeTaskExecution, shapes Options/OverwriteMode/TransferMode/PhaseStatus/TaskExecutionResultDetail/InvalidRequestException).

=== Cause 1 of 6: CreateLocationS3 does not perform the bucket-access test ===
AWS: Smithy models CreateLocationS3 with required S3BucketArn (pattern ^arn:...:s3:::[a-zA-Z0-9.\-_]{1,255}$) and required S3Config.BucketAccessRoleArn, and declares InvalidRequestException (client, HTTP 400) as its only client error. Beyond the modeled patterns, real AWS proves the bucket-access role can actually read the bucket before creating the location: it assumes the role and performs an S3 list against the bucket, and when the role's policy does not grant it, the call is rejected with HTTP 400 / error code InvalidRequestException and no location is created. Recorded behavior of the message (an example, not a requirement): "DataSync location access test failed: could not perform s3:ListObjectsV2 on bucket <bucket>. Access denied. Ensure bucket access role has s3:ListBucket permission." The requirement is the status, the InvalidRequestException code, and the absence of the location.
Emulator: the request succeeds. The check's _error() helper raised "the call succeeded; real AWS rejects it, so there is no error code to read" — a location was created for a bucket whose bucket-access role grants nothing for it, and it is then returned by ListLocations. The emulator validates only the ARN shape.
Why: a client that uses CreateLocationS3 as its pre-flight IAM check (the documented way to discover a mis-scoped bucket-access role before building a task) gets a green light against the emulator and only discovers the broken role much later, at execution time in real AWS. Negative-path tests of role scoping cannot be written at all.
Fix: the CreateLocationS3 handler's validation — it must evaluate the supplied BucketAccessRoleArn's effective S3 permissions against the named bucket in emulator IAM/S3 state, and fail with InvalidRequestException (400) without persisting a location.
Traces: datasync.create-location-s3-unreadable-bucket-rejected

=== Cause 2 of 6: CreateTask accepts DestinationLocationArn equal to SourceLocationArn ===
AWS: Smithy declares SourceLocationArn and DestinationLocationArn as two independently required LocationArn members with identical patterns; nothing in the model forbids them being equal, so this is a cross-member business rule. Real AWS refuses the pair with HTTP 400 / InvalidRequestException and creates no task. Recorded message, as an example: "Destination location cannot be same as source location."
Emulator: CreateTask returns 200 with a TaskArn for source == destination (check failed at _error() with "the call succeeded"), so a task that would copy a bucket onto itself exists and is listed.
Why: a client relying on AWS to reject the degenerate task sees it accepted and then started, so a configuration bug that AWS catches at CreateTask time silently becomes a running self-copy against the emulator.
Fix: cross-member validation in the CreateTask handler, before any task state is persisted.
Traces: datasync.create-task-rejects-same-source-and-destination-location

=== Cause 3 of 6: CreateTask accepts the contradictory Options pair TransferMode=ALL + OverwriteMode=NEVER ===
AWS: Smithy models Options.TransferMode (enum CHANGED|ALL) and Options.OverwriteMode (enum ALWAYS|NEVER) as independent closed enums, so each value is individually legal and a model-driven emulator accepts the pair. Real AWS rejects the COMBINATION with HTTP 400 / InvalidRequestException and creates no task, while each option on its own (TransferMode=CHANGED+OverwriteMode=NEVER, and TransferMode=ALL+OverwriteMode=ALWAYS) is accepted. Recorded message, as an example: "Invalid SyncOption value. Option: TransferMode,OverwriteMode, Value: ALL,NEVER."
Emulator: CreateTask returns 200 for the ALL+NEVER pair (check failed at _error() with "the call succeeded").
Why: a client that depends on AWS to surface the contradiction at create time gets a task whose two options cancel each other out, with no error anywhere in its lifecycle.
Fix: Options combination validation in the CreateTask handler (and the same rule in UpdateTask), emitting InvalidRequestException (400) with no task persisted.
Traces: datasync.create-task-rejects-transfer-all-with-overwrite-never

=== Cause 4 of 6: a task whose bucket-access role lost S3 access still ends in SUCCESS; executions can never reach ERROR ===
AWS: Smithy models DescribeTaskExecution.Status (TaskExecutionStatus) and Result (TaskExecutionResultDetail with PrepareStatus/TransferStatus/VerifyStatus of enum PhaseStatus PENDING|SUCCESS|ERROR, plus ErrorCode/ErrorDetail strings). When the role named by the task's locations can no longer read the source bucket, StartTaskExecution still returns 200 and the denial surfaces only in the execution: a terminal Status=ERROR with Result.PrepareStatus=ERROR, Result.TransferStatus=PENDING (the transfer phase never starts), a phase-level Result.ErrorCode (recorded value, as an example: "FsS3UnableToConnectToEndpoint"), FilesTransferred=0, BytesTransferred=0, an empty destination bucket, and the task itself still Status=AVAILABLE.
Emulator: the execution ends Status='SUCCESS' with the fixed Result {"PrepareDuration":300,"PrepareStatus":"SUCCESS","TotalDuration":1200,"TransferDuration":300,"TransferStatus":"SUCCESS","VerifyDuration":300,"VerifyStatus":"SUCCESS"} and no ErrorCode/ErrorDetail at all. The execution state machine has no failure path and never consults the bucket-access role.
Why: a client's failure handling — alarms on execution Status=ERROR, retry/rollback keyed off Result.ErrorCode, dashboards on per-phase status — is completely unexercisable: every execution reports success regardless of whether the data could be read, so an emulator-backed test suite cannot cover the single most common DataSync production failure.
Fix: the task-execution state machine/state (the component that advances an execution through PREPARING/TRANSFERRING/VERIFYING and writes TaskExecutionResultDetail): the prepare phase must resolve the locations' BucketAccessRoleArn against emulator IAM/S3 state and, on denial, terminate the execution with Status=ERROR, PrepareStatus=ERROR, TransferStatus=PENDING and a populated Result.ErrorCode, leaving counters at 0 and the task AVAILABLE.
Traces: datasync.execution-denied-when-role-loses-source-bucket-access

=== Cause 5 of 6: OverwriteMode=NEVER is not honored by the execution ===
AWS: Options.OverwriteMode=NEVER (Smithy enum ALWAYS|NEVER) means DataSync must not replace a destination object that already exists. With a CHANGED transfer onto a destination whose objects all already exist with different content, real AWS leaves every destination object byte-for-byte as it was, counts all of them in FilesSkipped with BytesWritten=0, reports Result.TransferStatus=SUCCESS (the files are skipped, not failed) and Result.VerifyStatus=ERROR, and the execution reaches terminal Status=ERROR with a populated Result.ErrorCode (recorded value as an example: "CompleteWithErrorNoLogsWithVerificationError", detail "Transfer and verification completed. Verification detected mismatches. ...") because verification legitimately finds destination != source. The identical task with OverwriteMode=ALWAYS instead ends SUCCESS and replaces the bytes.
Emulator: the NEVER execution ends Status='SUCCESS' with the same fixed all-SUCCESS Result shown in Cause 4 and no ErrorCode — the option is treated as advisory and the NEVER/ALWAYS contrast does not exist.
Why: a client using OverwriteMode=NEVER to protect pre-existing destination data cannot verify that protection, and cannot test the verification-mismatch error path that NEVER predictably produces; against the emulator a run that AWS reports as failed verification looks like a clean success.
Fix: the execution engine's transfer/verify phases must branch on the task's Options.OverwriteMode — skip existing destination keys (counting FilesSkipped, BytesWritten=0) and let the verify phase fail the execution with VerifyStatus=ERROR and a Result.ErrorCode.
Traces: datasync.overwrite-mode-never-preserves-existing-destination-objects

=== Cause 6 of 6: a successful S3->S3 execution reports zero counters and copies no object bytes ===
AWS: DescribeTaskExecution's EstimatedFilesToTransfer, EstimatedBytesToTransfer, FilesTransferred, BytesTransferred, BytesWritten and FilesVerified (Smithy: long, default 0) describe the work the execution actually did. For an agentless S3->S3 ALL-mode task over a source bucket holding N objects totalling B bytes, real AWS reports EstimatedFilesToTransfer=N, EstimatedBytesToTransfer=B, FilesTransferred=N, BytesTransferred=B, BytesWritten=B, FilesVerified=N, FilesDeleted=0, and the destination bucket afterwards holds those objects with the source content.
Emulator: the execution reports Status=SUCCESS but EstimatedFilesToTransfer=0 where 3 source objects were staged ("AssertionError: EstimatedFilesToTransfer was 0, expected 3"); the counters are left at their modeled defaults and the destination side-effect is not produced. The result detail is the same fixed stub as in Cause 4.
Why: every client observation of a DataSync transfer is broken — progress and volume reporting read 0 for a run reported as successful, and any downstream consumer of the destination bucket (the normal reason to run DataSync at all) finds it empty, so no end-to-end pipeline can be exercised against the emulator.
Fix: the execution engine's prepare/transfer/verify phases and the DescribeTaskExecution serializer: enumerate the source location's objects to populate the Estimated* counters, perform the copy into the destination location, and report FilesTransferred/BytesTransferred/BytesWritten/FilesVerified from the work actually done.
Traces: datasync.s3-to-s3-agentless-transfer-copies-objects-and-counters

No open DataSync issue exists (list_existing_issues query='datasync' returned none). The two remaining datasync failures in this run (already-synced-destination-transfers-no-bytes-but-verifies, simple-pattern-filters-partition-the-transferred-set) are marked OWNED by 20261022T154846Z-emr-glue-s3-cross-service-suite-cfn-declared.md (S3 ListObjectsV2 with encoding-type scaffolded) and are deliberately not filed here.

## Evidence
Suite: simparity-behavioral-aws-v1, datasync 0/8 (0%).

1) datasync.create-location-s3-unreadable-bucket-rejected [mismatch]
   Expected: HTTP 400, Error.Code=InvalidRequestException, no location created for the unreadable bucket (ListLocations shows only the readable one).
   Actual: CreateLocationS3 returned success. AssertionError: "the call succeeded; real AWS rejects it, so there is no error code to read" (check line 48 -> _error line 91).

2) datasync.create-task-rejects-same-source-and-destination-location [mismatch]
   Expected: HTTP 400, Error.Code=InvalidRequestException for SourceLocationArn == DestinationLocationArn; no task left behind.
   Actual: CreateTask returned a TaskArn. AssertionError: "the call succeeded; real AWS rejects it, so there is no error code to read" (check line 34 -> _error line 82).

3) datasync.create-task-rejects-transfer-all-with-overwrite-never [mismatch]
   Expected: HTTP 400, Error.Code=InvalidRequestException for Options{TransferMode=ALL, OverwriteMode=NEVER}; each option alone accepted.
   Actual: CreateTask returned a TaskArn. AssertionError: "the call succeeded; real AWS rejects it, so there is no error code to read" (check line 35 -> _error line 103).

4) datasync.execution-denied-when-role-loses-source-bucket-access [mismatch]
   Expected: execution Status=ERROR, Result.PrepareStatus=ERROR, Result.TransferStatus=PENDING, Result.ErrorCode present, FilesTransferred=0, BytesTransferred=0, destination empty, task still AVAILABLE.
   Actual: AssertionError: execution with no bucket access ended 'SUCCESS', expected ERROR (result {"PrepareDuration": 300, "PrepareStatus": "SUCCESS", "TotalDuration": 1200, "TransferDuration": 300, "TransferStatus": "SUCCESS", "VerifyDuration": 300, "VerifyStatus": "SUCCESS"}) — no ErrorCode/ErrorDetail.

5) datasync.overwrite-mode-never-preserves-existing-destination-objects [mismatch]
   Expected: OverwriteMode=NEVER execution Status=ERROR with Result.TransferStatus=SUCCESS, Result.VerifyStatus=ERROR, Result.ErrorCode present, FilesSkipped=3, BytesWritten=0, destination objects unchanged; OverwriteMode=ALWAYS control SUCCESS with BytesWritten=850.
   Actual: AssertionError: OverwriteMode NEVER ended 'SUCCESS', expected ERROR (result {"PrepareDuration": 300, "PrepareStatus": "SUCCESS", "TotalDuration": 1200, "TransferDuration": 300, "TransferStatus": "SUCCESS", "VerifyDuration": 300, "VerifyStatus": "SUCCESS"}).

6) datasync.s3-to-s3-agentless-transfer-copies-objects-and-counters [mismatch]
   Expected: EstimatedFilesToTransfer=3, EstimatedBytesToTransfer=850, FilesTransferred=3, BytesTransferred=850, BytesWritten=850, FilesVerified=3, FilesDeleted=0, destination holding the 3 source objects with source bytes.
   Actual: AssertionError: EstimatedFilesToTransfer was 0, expected 3 (check line 70) — counters left at the modeled default 0 while Status=SUCCESS.

Model references: CreateTask errors = [InternalException(500), InvalidRequestException(400, client)]; CreateLocationS3 errors identical; DescribeTaskExecution counters are long with default 0; Options.TransferMode enum [CHANGED, ALL]; Options.OverwriteMode enum [ALWAYS, NEVER]; TaskExecutionResultDetail members PrepareStatus/TransferStatus/VerifyStatus target PhaseStatus enum [PENDING, SUCCESS, ERROR] plus ErrorCode/ErrorDetail strings.

## Steps to Reproduce
Boot the emulator and run the datasync behavioral checks (all use only datasync, s3 and iam):

Cause 1: create two S3 buckets, put an object in each, create an IAM role whose inline S3 policy names ONLY the first bucket, then CreateLocationS3 for the first bucket (succeeds — positive control) and CreateLocationS3 for the second with the same S3Config.BucketAccessRoleArn. Observe the second call returns 200 instead of 400/InvalidRequestException, and ListLocations reports both. Replay: datasync.create-location-s3-unreadable-bucket-rejected

Cause 2: create two S3 locations, then CreateTask with SourceLocationArn == DestinationLocationArn and Options{VerifyMode=POINT_IN_TIME_CONSISTENT, LogLevel=OFF, TransferMode=ALL}. Observe a TaskArn is returned instead of 400/InvalidRequestException, and ListTasks contains the refused name. Replay: datasync.create-task-rejects-same-source-and-destination-location

Cause 3: with two distinct S3 locations, CreateTask with Options{VerifyMode=POINT_IN_TIME_CONSISTENT, LogLevel=OFF, TransferMode=ALL, OverwriteMode=NEVER}. Observe 200 instead of 400/InvalidRequestException. Replay: datasync.create-task-rejects-transfer-all-with-overwrite-never

Cause 4: create a source bucket with 3 objects plus a destination bucket, create a role granting both, create the two S3 locations and a task, then DeleteRolePolicy to strip the role's S3 policy, StartTaskExecution and poll DescribeTaskExecution to a terminal status. Observe Status=SUCCESS with all phase statuses SUCCESS and no Result.ErrorCode, instead of Status=ERROR / PrepareStatus=ERROR / TransferStatus=PENDING. Replay: datasync.execution-denied-when-role-loses-source-bucket-access

Cause 5: seed a source bucket with 3 objects and a destination bucket with the SAME 3 keys holding different content; create a task with Options{TransferMode=CHANGED, OverwriteMode=NEVER}; StartTaskExecution and poll to terminal. Observe Status=SUCCESS (no VerifyStatus=ERROR, no ErrorCode) and no FilesSkipped/BytesWritten distinction from the OverwriteMode=ALWAYS control. Replay: datasync.overwrite-mode-never-preserves-existing-destination-objects

Cause 6: seed a source bucket with 3 objects (100 + 250 + 500 bytes), create source/destination S3 locations and a task with Options{VerifyMode=POINT_IN_TIME_CONSISTENT, LogLevel=OFF, TransferMode=ALL}, StartTaskExecution, poll DescribeTaskExecution to terminal. Observe Status=SUCCESS but every counter 0 and the destination bucket not holding the source objects. Replay: datasync.s3-to-s3-agentless-transfer-copies-objects-and-counters
