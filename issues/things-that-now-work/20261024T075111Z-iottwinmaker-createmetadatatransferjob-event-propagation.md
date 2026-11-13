Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# A metadata transfer job never transfers anything and always settles in ERROR/PROCESSING_ERROR instead of COMPLETED

_Filed: 2026-10-24T07:51:11Z_
_Operation: CreateMetadataTransferJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateMetadataTransferJob is the documented bulk import/export path: the service reads the job's source (an Amazon S3 metadata document, an IoT SiteWise asset-model hierarchy, or another workspace) and writes what it describes into the destination (component types/entities in the destination workspace, or a metadata document in the destination S3 bucket), reporting progress counts and a reportUrl as the job moves VALIDATING -> RUNNING -> COMPLETED.

- Expected: for a well-formed job (e.g. source type `s3` naming a valid metadata document, destination type `iottwinmaker` naming an existing workspace), polling GetMetadataTransferJob eventually reports `status.state = COMPLETED`, a populated `progress` (totalCount/succeededCount/...), a `reportUrl`, and the destination workspace gains the entities/component types the document described (visible via GetEntity/ListEntities/ListComponentTypes).
- Actual: the job is accepted in VALIDATING and, 500 ms later, any read materialises the hardcoded terminal failure `status.state = ERROR` with `error.code = PROCESSING_ERROR` and an emulator-authored message ("this emulator transfers no metadata..."). Nothing is ever read from the source or written to the destination, `progress` and `reportUrl` are hardcoded absent, and no state in the destination workspace changes. A caller that waits for COMPLETED waits forever and a caller that imports metadata gets an empty workspace plus a failure it cannot provoke in AWS.

## Evidence

Spec: the model declares `MetadataTransferJobState` including COMPLETED/RUNNING, `MetadataTransferJobProgress`, and `reportUrl` on GetMetadataTransferJobResponse — all unreachable here. Handler/state: `services/.../src/state.rs` — `METADATA_TRANSFER_JOB_ERROR_CODE = "PROCESSING_ERROR"`, `METADATA_TRANSFER_JOB_ERROR_MESSAGE` ("this emulator transfers no metadata: nothing was read from the job's sources and nothing was written to its destination"), and `MetadataTransferJob::settle()` which unconditionally writes `METADATA_TRANSFER_JOB_STATE_ERROR` once `settles_at` passes; `src/ops/create_metadata_transfer_job.rs:292-305` only validates and persists the request, performing no transfer; `src/ops/get_metadata_transfer_job.rs:210,222,230` settles then reports `report_url: None` and `progress: None`. Tests: `tests/all/metadata_transfer_job.rs:360-397` ASSERTS the ERROR/PROCESSING_ERROR outcome, i.e. the divergence is locked in by the suite rather than covered by it.

## Steps to Reproduce

1. CreateWorkspace("ws-dest") and upload (or assume) a valid IoT TwinMaker metadata document in S3.
2. CreateMetadataTransferJob(sources=[{type:"s3", s3Configuration:{location:"arn:aws:s3:::<bucket>/<key>"}}], destination={type:"iottwinmaker", iotTwinMakerConfiguration:{workspace:<ws-dest arn>}}) -> 200 with status.state VALIDATING.
3. Poll GetMetadataTransferJob(metadataTransferJobId=<id>) for a few seconds. Expected: state RUNNING then COMPLETED, with `progress` counts and a `reportUrl`. Actual: state ERROR with error.code PROCESSING_ERROR, progress and reportUrl absent.
4. ListComponentTypes/ListEntities on "ws-dest". Expected: the component types and entities described by the metadata document. Actual: the workspace is unchanged (empty).
