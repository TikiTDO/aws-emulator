Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__kms__s3.minimum-encryption-configuration-rejects-a-weaker-client-encryption-option
Issue-Key: athena__kms__s3.minimum-encryption-configuration-rejects-a-weaker-client-encryption-option|run#23|KeyError|assert head["ServerSideEncryption"] == "aws:kms", (
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# athena__kms__s3: Athena query results are written to S3 with no server-side encryption, so the workgroup/request SSE_KMS EncryptionConfiguration is never applied

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Cause 1 of 1: the result object Athena writes for a SUCCEEDED query carries no server-side encryption, although the execution's resolved ResultConfiguration asked for SSE_KMS with a customer-managed CMK.

AWS: StartQueryExecution's ResultConfiguration.EncryptionConfiguration (Smithy: com.amazonaws.athena#ResultConfiguration$EncryptionConfiguration -> com.amazonaws.athena#EncryptionConfiguration, with required member EncryptionOption of enum com.amazonaws.athena#EncryptionOption values SSE_S3 | SSE_KMS | CSE_KMS, plus KmsKey = "the KMS key ARN or ID") selects how Athena stores the query's results. With EncryptionOption=SSE_KMS and KmsKey=<CMK ARN>, Athena writes both `<prefix>/<QueryExecutionId>.csv` and its `<QueryExecutionId>.csv.metadata` sibling as SSE-KMS objects: a subsequent S3 HeadObject on either key answers 200 with x-amz-server-side-encryption: aws:kms (botocore `ServerSideEncryption` == "aws:kms") and x-amz-server-side-encryption-aws-kms-key-id set to the resolved key ARN (botocore `SSEKMSKeyId`). With EncryptionOption=SSE_S3 the same HeadObject reports ServerSideEncryption "AES256" and no SSEKMSKeyId. The encryption option is part of the modeled, client- or workgroup-selected result configuration, so it is a requirement of the write, not a per-run value.

Emulator: after a query submitted with WorkGroup=<wg> and ResultConfiguration={OutputLocation: s3://<bucket>/results/, EncryptionConfiguration: {EncryptionOption: SSE_KMS, KmsKey: <CMK ARN>}} reaches SUCCEEDED, the result object does exist — HeadObject on `results/<QueryExecutionId>.csv` answers 200 — but the response carries NO ServerSideEncryption member at all (botocore raises KeyError: 'ServerSideEncryption'), and therefore no SSEKMSKeyId either. The bytes are stored unencrypted/with no recorded SSE state, i.e. the resolved EncryptionConfiguration is dropped on the S3 write (or the recorded SSE state is not projected back by HeadObject).

Why: a client that relies on Athena result encryption cannot verify or obtain it. Compliance tooling that heads/lists result objects and asserts aws:kms (and the expected CMK) concludes the results are unencrypted; a consumer that decrypts via KMS, or a bucket policy / SCP that requires s3:x-amz-server-side-encryption=aws:kms on result objects, behaves differently against the emulator than against AWS. It also makes every workgroup result-encryption control (including EnableMinimumEncryptionConfiguration, whose whole purpose is to floor this option) unobservable: the check that exercises the floor aborts here, before the below-floor refusal (InvalidRequestException with AWS's "The encryption option in the request does not meet the minimum encryption option specified in the workgroup. The valid option(s) are: [SSE_KMS, CSE_KMS].") is ever reached, so that part of the contract is untested rather than known-good.

Fix: the athena crate's query-execution result writer — it must carry the resolved ResultConfiguration/WorkGroupConfiguration EncryptionConfiguration (EncryptionOption + KmsKey) into the S3 put of both the `<QueryExecutionId>.csv` object and its `.csv.metadata` sibling, so the stored object records aws:kms with the resolved key ARN for SSE_KMS and AES256 for SSE_S3; and, if the SSE state is recorded but not returned, the S3 object store's HeadObject projection of x-amz-server-side-encryption / x-amz-server-side-encryption-aws-kms-key-id. Related but NOT covering this: 20261104T163600Z-athena-writes-no-query-result-object-to-s3.md (result object missing entirely / output-location resolution) and 20261107T185637Z-kms-s3-sse-kms-is-a-stored-header-not-envelope.md (SSE-KMS stored as a header rather than envelope encryption) — here the object is written and headed successfully but has no SSE state at all; 20261111T024602Z-kms-lambda-s3-s3-getobject-omits-x-amz.md concerns GetObject's key-id header on an object HeadObject does report as SSE-KMS.

Traces: athena__kms__s3.minimum-encryption-configuration-rejects-a-weaker-client-encryption-option.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

[error] athena__kms__s3.minimum-encryption-configuration-rejects-a-weaker-client-encryption-option:
  File "check_athena__kms__s3_minimum_encryption_configuration_rejects_a_weaker_client_encryption_option.py", line 161, in run
    assert head["ServerSideEncryption"] == "aws:kms", (
  KeyError: 'ServerSideEncryption'

The check had already established the preconditions before this point: kms.CreateKey + DescribeKey reported Enabled=true; CreateWorkGroup(Configuration={ResultConfiguration={OutputLocation: s3://<bucket>/results/, EncryptionConfiguration: {EncryptionOption: SSE_KMS, KmsKey: <key ARN>}}, EnforceWorkGroupConfiguration: False, EnableMinimumEncryptionConfiguration: True}) and GetWorkGroup echoed both the floor flag and EncryptionOption=SSE_KMS; StartQueryExecution(QueryString="SELECT 'enc-<marker>' AS marker", WorkGroup=<wg>, ResultConfiguration={OutputLocation: s3://<bucket>/results/, EncryptionConfiguration: {EncryptionOption: SSE_KMS, KmsKey: <key ARN>}}) reached SUCCEEDED.

Expected: s3.head_object(Bucket=<bucket>, Key='results/<QueryExecutionId>.csv') -> 200 with ServerSideEncryption='aws:kms' and SSEKMSKeyId=<the CMK ARN used for the query>; the same for 'results/<QueryExecutionId>.csv.metadata'.
Actual: head_object succeeded (the object exists) but the response contains neither ServerSideEncryption nor SSEKMSKeyId — the result bytes were written with no server-side encryption recorded.

Not reached because of this failure, hence not claimed as divergences here: the below-floor StartQueryExecution refusal (InvalidRequestException, message "The encryption option in the request does not meet the minimum encryption option specified in the workgroup. The valid option(s) are: [SSE_KMS, CSE_KMS]."), the "a refused query writes nothing" effect check, and the floor-dropped SSE_S3 re-run that must store AES256 with no SSEKMSKeyId.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect athena__kms__s3.minimum-encryption-configuration-rejects-a-weaker-client-encryption-option.
3. Minimal manual repro:
   a. kms.CreateKey(KeyUsage='ENCRYPT_DECRYPT') -> note KeyMetadata.Arn (K); confirm DescribeKey reports Enabled=true.
   b. s3.CreateBucket(B) (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
   c. athena.CreateWorkGroup(Name=W, Configuration={'ResultConfiguration': {'OutputLocation': 's3://B/results/', 'EncryptionConfiguration': {'EncryptionOption': 'SSE_KMS', 'KmsKey': K}}, 'EnforceWorkGroupConfiguration': False, 'EnableMinimumEncryptionConfiguration': True}).
   d. qid = athena.StartQueryExecution(QueryString="SELECT 'x' AS marker", WorkGroup=W, ResultConfiguration={'OutputLocation': 's3://B/results/', 'EncryptionConfiguration': {'EncryptionOption': 'SSE_KMS', 'KmsKey': K}}); poll GetQueryExecution to SUCCEEDED.
   e. s3.HeadObject(Bucket=B, Key=f'results/{qid}.csv') — expect ServerSideEncryption='aws:kms' and SSEKMSKeyId=K; observe a response with no ServerSideEncryption / SSEKMSKeyId at all. Repeat for f'results/{qid}.csv.metadata'.
   f. For the other half of the contract: athena.UpdateWorkGroup(WorkGroup=W, ConfigurationUpdates={'EnableMinimumEncryptionConfiguration': False}) and rerun the query with EncryptionConfiguration={'EncryptionOption': 'SSE_S3'} — HeadObject must then report ServerSideEncryption='AES256' and no SSEKMSKeyId.
