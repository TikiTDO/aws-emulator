Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medical-imaging.dicom-import-job-success-creates-active-image-set medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata medical-imaging.update-image-set-metadata-updates-patient-name medical-imaging.update-image-set-metadata-concurrent-conflict
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging.dicom-import-job-success-creates-active-image-set|_find_image_set_id#17|AssertionError|raise AssertionError("import COMPLETED but no image set was ever indexed for the study")
Issue-Key: medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata|_find_image_set_id#17|AssertionError|raise AssertionError("import COMPLETED but no image set was ever indexed for the study")
Issue-Key: medical-imaging.update-image-set-metadata-updates-patient-name|_find_image_set_id#17|AssertionError|raise AssertionError("import COMPLETED but no image set was ever indexed for the study")
Issue-Key: medical-imaging.update-image-set-metadata-concurrent-conflict|_find_image_set_id#17|AssertionError|raise AssertionError("import COMPLETED but no image set was ever indexed for the study")
Issue-Commit: 0fa755cca03015f03b3e195a115fe9a7959107cb

# medical-imaging: a COMPLETED DICOM import job indexes no image set, so SearchImageSets stays empty and the whole image-set data plane is unreachable

> _Note: this issue was identified against the code base at commit `0fa755cca03015f03b3e195a115fe9a7959107cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDICOMImportJob

## Description
Behavioral replay of AWS HealthImaging (medical-imaging) left 8 of 18 checks failing. Four of them are marked OWNED by the open bundle 20261029T201912Z-medical-imaging-imported-image-sets-never-materialise-deletedatastore.md and are skipped here. The remaining four all fail on ONE root cause, described below. Note: that same open bundle's Cause 1 describes this behavior, and the open issue 20261024T071454Z-medical-imaging-startdicomimportjob-wrong-behavior.md describes it from the source side, but neither carries the replay keys these four checks now fail with (they previously died on the harness's 60s budget inside the discovery loop; they now reach the loop's explicit assertion instead), so the cause is re-filed here with the current grounding.

=== Cause 1 of 1: a DICOM import that reaches jobStatus COMPLETED creates no image set ===
Failing checks: medical-imaging.dicom-import-job-success-creates-active-image-set, medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata, medical-imaging.update-image-set-metadata-updates-patient-name, medical-imaging.update-image-set-metadata-concurrent-conflict.

AWS: Smithy: StartDICOMImportJob (POST /startDICOMImportJob/datastore/{datastoreId}, required dataAccessRoleArn/clientToken/inputS3Uri/outputS3Uri) is documented "Start importing bulk data into an ACTIVE data store. The import job imports DICOM P10 files found in the S3 prefix specified by the inputS3Uri parameter". The model declares the artifact that import produces: SearchImageSets (POST /datastore/{datastoreId}/searchImageSets) returns a required imageSetsMetadataSummaries list; GetImageSet returns required imageSetId/versionId/imageSetState (enum ImageSetState) plus imageSetWorkflowStatus (enum ImageSetWorkflowStatus, which declares IMPORTING/IMPORTED); GetImageSetMetadata serves the normalized DICOM metadata; GetImageFrame serves imageFrameBlob with a Content-Type header; CopyImageSet and UpdateImageSetMetadata address an existing imageSetId as an @httpLabel. So a job whose GetDICOMImportJob reports jobStatus COMPLETED leaves at least one image set in the target data store — imageSetState ACTIVE, imageSetWorkflowStatus IMPORTED — discoverable by SearchImageSets with an EQUAL filter on DICOMStudyInstanceUID for the study that was imported, carrying the Patient/Study metadata of the imported P10 file and its image frames.

Emulator: StartDICOMImportJob mints an ImportJob, inserts it into datastore.import_jobs and touches nothing else; the clock then marches the job SUBMITTED -> IN_PROGRESS -> COMPLETED without creating anything. SearchImageSets on the imported study keeps answering HTTP 200 with an empty imageSetsMetadataSummaries forever, so each of the four checks exhausts its image-set discovery loop and raises AssertionError: "import COMPLETED but no image set was ever indexed for the study". Because the first image set can never come into existence, CopyImageSet, UpdateImageSetMetadata (including its latestVersionId optimistic-concurrency / ConflictException behavior) and GetImageFrame cannot be reached through the HTTP surface at all — they are not merely unverified.

Why: a client cannot perform the workflow a HealthImaging data store exists for. It imports, observes COMPLETED, and then finds nothing: search returns no summaries, so there is no imageSetId to pass to GetImageSet/GetImageSetMetadata/GetImageFrame/CopyImageSet/UpdateImageSetMetadata. Code that imports and then reads, copies or versions an image set dead-ends on an empty search result that AWS never returns for a completed import, and the concurrency contract of UpdateImageSetMetadata can never be exercised.

Fix: the import job's completion path in the state module that owns the clock transition (the ImportJob advance that today only rewrites job_status), together with the StartDICOMImportJob handler, which currently records only the job. Completing a job must materialise the image set(s) the job was asked to import, their normalized DICOM metadata (indexed so SearchImageSets' DICOMStudyInstanceUID filter matches) and their image frames, with imageSetState ACTIVE and imageSetWorkflowStatus IMPORTED, so the image-set operations have a subject.

Traces: failing checks medical-imaging.dicom-import-job-success-creates-active-image-set, medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata, medical-imaging.update-image-set-metadata-updates-patient-name, medical-imaging.update-image-set-metadata-concurrent-conflict. Source: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/start_dicom_import_job.rs lines 255-294 build an ImportJob and insert it into datastore.import_jobs, with no image-set write anywhere on the path; src/state.rs IMPORT_JOB_IN_PROGRESS_DELAY / IMPORT_JOB_COMPLETION_DELAY only move the status.

## Evidence
Suite: medical-imaging 10/18 passed; 8 mismatched, 4 of them OWNED and skipped. The four clustered here:

  [mismatch] medical-imaging.dicom-import-job-success-creates-active-image-set
    File "check_medical_imaging_dicom_import_job_success_creates_active_image_set.py", line 271, in run
      image_set_id = _find_image_set_id(mi, datastore_id, study_uid)
    File ... line 186, in _find_image_set_id
      raise AssertionError("import COMPLETED but no image set was ever indexed for the study")
    AssertionError: import COMPLETED but no image set was ever indexed for the study

  [mismatch] medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata
    File ... line 272, in run -> _find_image_set_id ... line 183
    AssertionError: import COMPLETED but no image set was ever indexed for the study

  [mismatch] medical-imaging.update-image-set-metadata-updates-patient-name
    File ... line 280, in run -> _find_image_set_id ... line 189
    AssertionError: import COMPLETED but no image set was ever indexed for the study

  [mismatch] medical-imaging.update-image-set-metadata-concurrent-conflict
    File ... line 292, in run -> _find_image_set_id ... line 196
    AssertionError: import COMPLETED but no image set was ever indexed for the study

Expected (AWS): after GetDICOMImportJob reports jobStatus COMPLETED, SearchImageSets with searchCriteria filters [{operator: EQUAL, values: [{DICOMStudyInstanceUID: <the imported study UID>}]}] returns imageSetsMetadataSummaries with at least one entry whose imageSetId then resolves through GetImageSet (imageSetState ACTIVE, imageSetWorkflowStatus IMPORTED).
Actual (emulator): HTTP 200 with imageSetsMetadataSummaries == [] on every poll, indefinitely. The study UID, bucket, role name, datastoreId and jobId are per-run values and are not part of the requirement; the requirement is that a COMPLETED import yields a searchable, ACTIVE image set for the study it imported.

That the failure is in the post-import phase and not in setup is shown by the sibling medical-imaging.delete-datastore-conflicts-with-active-import-job check, which performs the identical setup (CreateDatastore, wait ACTIVE, S3 bucket, IAM role + inline policy, StartDICOMImportJob) and reaches its own assertion normally.

## Steps to Reproduce
Mount the medical-imaging, s3, iam and sts emulators on one router and replay the four checks in trace_ids. By hand:

1. POST /datastore with a clientToken; poll GET /datastore/{datastoreId} until datastoreStatus is ACTIVE.
2. Create an S3 bucket and PUT a minimal DICOM P10 object under in/ (with StudyInstanceUID (0020,000D) set to a known UID).
3. Create an IAM role whose trust policy allows medical-imaging.amazonaws.com to assume it, with s3:ListBucket/GetObject/PutObject on that bucket.
4. POST /startDICOMImportJob/datastore/{datastoreId} with dataAccessRoleArn=<that role>, inputS3Uri=s3://<bucket>/in/, outputS3Uri=s3://<bucket>/out/, clientToken=<token>.
5. Poll GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId} until jobStatus is COMPLETED.
6. POST /datastore/{datastoreId}/searchImageSets with searchCriteria {"filters": [{"operator": "EQUAL", "values": [{"DICOMStudyInstanceUID": "<the UID from step 2>"}]}]}.

Observed: step 6 answers 200 with imageSetsMetadataSummaries == [] on every attempt, so there is no imageSetId to pass to POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSet, .../getImageSetMetadata, .../getImageFrame, .../copyImageSet or .../updateImageSetMetadata.
Expected: step 6 returns at least one summary for the imported study, and GetImageSet on it reports imageSetState ACTIVE with imageSetWorkflowStatus IMPORTED.
