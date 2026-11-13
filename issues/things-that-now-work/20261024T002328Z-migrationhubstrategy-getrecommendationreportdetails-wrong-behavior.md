Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# A completed report generation task reports no s3Bucket/s3Keys and a statusMessage naming the emulator

_Filed: 2026-10-24T00:23:28Z_
_Operation: GetRecommendationReportDetails | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

After a report generation task reaches its terminal SUCCESS status, AWS's GetRecommendationReportDetails reports WHERE the generated report file is: RecommendationReportDetails$s3Bucket ('The S3 bucket where the report file is located') and RecommendationReportDetails$s3Keys ('The Amazon S3 key name of the report file') are populated for the account's report bucket (the s3bucketForReportData named at assessment/import start), and statusMessage is the service's own progress text.

The emulator reports status SUCCESS with s3Bucket and s3Keys ALWAYS absent (src/state.rs::RecommendationReport::to_details hardcodes s3_bucket: None, s3_keys: None), and fills statusMessage with emulator-authored text that names the emulator itself ('... no report file was written: this emulator serves no Amazon S3 data plane to upload one to.'). So a client that follows the documented flow — start generation, poll until SUCCESS, then fetch the report object from (s3Bucket, s3Keys) — has nothing to fetch and no error to branch on, and the message leaks implementation detail onto the wire. The repository does emulate S3, so a key can be written to the configured report bucket rather than reported as unwritable.

## Evidence

Smithy model: com.amazonaws.migrationhubstrategy#RecommendationReportDetails declares status, statusMessage, startTime, completionTime, s3Bucket and s3Keys; s3Bucket/s3Keys document the location of the generated report file, and the task's terminal status is SUCCESS (RecommendationReportStatus). Handler/state: src/ops/get_recommendation_report_details.rs returns report.to_details(); src/state.rs::to_details (lines ~442-453) sets s3_bucket: None / s3_keys: None and the REPORT_SUCCESS / settle() message text is the emulator-authored sentence above. Tests: tests/all/recommendation_report.rs::a_settled_task_reports_its_terminal_status_and_its_scope ASSERTS s3Bucket and s3Keys are absent for a settled task, so the gap is locked in rather than covered. A previously fixed issue for this service (20261022T103621Z-...-validation-and-not-found-responses-carry-emulator-authored) established that this service's wire messages must not be emulator-authored; statusMessage was not in its scope.

## Steps to Reproduce

1. POST /start-recommendation-report-generation with {"outputFormat": "Json"} -> 200 with an id.
2. Wait past the task's completion span and GET /get-recommendation-report-details/{id}.
3. Expected: recommendationReportDetails.status == SUCCESS with s3Bucket naming the account's report bucket and s3Keys listing the generated report object key. Actual: status SUCCESS, s3Bucket and s3Keys absent.
4. Inspect recommendationReportDetails.statusMessage: it reports 'no report file was written: this emulator serves no Amazon S3 data plane to upload one to', text no AWS response can contain.
