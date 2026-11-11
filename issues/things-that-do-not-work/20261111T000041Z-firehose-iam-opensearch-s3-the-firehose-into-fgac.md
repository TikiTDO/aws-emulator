Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents
Issue-Key: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents|_wait_admin#18|Timeout|raise AssertionError(f"the IAM admin never got 200 on /_cluster/health within {timeout_s}s "
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# firehose__iam__opensearch__s3: the Firehose-into-FGAC-OpenSearch strict-mapping scenario never reaches a verdict — the check hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: the Firehose -> OpenSearch delivery scenario makes no observable progress and is cut off at the suite's 60 s cap, so neither the conforming-document delivery nor the failed-document S3 backup can be graded.
AWS: recorded behavior only for the end-to-end timing; the contract the check asserts is documented (Firehose developer guide, create-configure-backup / basic-deliver): a delivery stream with an AmazonopensearchserviceDestinationConfiguration, zero-second buffering, no retries and S3BackupMode=FailedDocumentsOnly delivers each record put with PutRecordBatch (FailedPutCount 0) into a date-suffixed index on the target domain, documents that violate a strict index template mapping are NOT indexed and are instead written to the S3 backup bucket under the error prefix, and DescribeDeliveryStream echoes the configured buffering/retry/backup settings.
Emulator: the check reaches no assertion before the cap ('Check timed out after 60.0s'). Its only unbounded waits are: the domain publishing an endpoint and admitting the assumed FGAC admin role, DescribeDeliveryStream reporting DeliveryStreamStatus=ACTIVE, and the delivered documents appearing via _cat/indices / _count on the domain. Hypothesis: Firehose delivery to an OpenSearch destination has no delivery path (the same inert-destination class already filed for the Splunk, HTTP-endpoint and Iceberg destinations, e.g. 20261102T112723Z-firehose-the-splunk-destination-has-no-delivery-path.md), so the index never appears and the poll cannot terminate; the related open issues cover other destinations, not OpenSearch.
Why: a client building the standard log/search ingestion pipeline (producer -> Firehose -> OpenSearch index, failures diverted to S3) observes an empty domain and an empty backup prefix, with no error anywhere to explain it; both the success and the failure halves of the contract are unobservable.
Fix: the Firehose delivery engine in the firehose service crate — the OpenSearch (AmazonopensearchserviceDestinationConfiguration) destination needs an actual delivery attempt against the domain's data plane, with mapping-rejected documents routed to the S3 backup prefix.
Traces: firehose__iam__opensearch__s3.xsvc-opensearch-1-firehose-fgac-strict-mapping-indexes-only-conforming-documents.

## Evidence
Replay output: 'Error: Check timed out after 60.0s' with no assertion reached. Expected: after PutRecordBatch of 7 documents (5 conforming, 2 violating a dynamic:strict template) the domain holds exactly one date-suffixed index named <prefix>-<date> with _count 5, the 2 rejected documents appear as objects under the S3 backup error prefix, and DescribeDeliveryStream echoes BufferingHints 0, RetryOptions 0 and S3BackupMode=FailedDocumentsOnly. Actual: nothing is indexed and the check is cut off at the cap.

## Steps to Reproduce
1. Create an IAM admin role, an FGAC OpenSearch domain mapping that role, and an S3 backup bucket plus a Firehose delivery role.
2. CreateDeliveryStream with an OpenSearch destination (index <prefix>, IndexRotationPeriod daily, zero buffering, no retries, S3BackupMode=FailedDocumentsOnly) and wait for DeliveryStreamStatus=ACTIVE.
3. PUT a dynamic:strict index template on the domain as the admin role.
4. PutRecordBatch 5 conforming and 2 conflicting documents; FailedPutCount must be 0.
5. Poll GET /_cat/indices?format=json and /<index>/_count on the domain, and ListObjectsV2 on the backup prefix — AWS shows 5 indexed and 2 backed up; the emulator shows neither before the budget expires.
