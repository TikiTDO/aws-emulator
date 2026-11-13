Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive
Issue-Key: emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# An S3 Batch Operations S3PutObjectTagging job never reaches a terminal status, so the batchoperations.s3 role is never assumed and the manifest's object is never tagged (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1: a submitted S3 Batch Operations job never settles into a terminal status and its tasks are never executed, so the job's RoleArn is never actually assumed and the manifest's target object is never tagged.

AWS: Smithy com.amazonaws.s3control#CreateJob (POST /v20180820/jobs, 200, returns JobId) and DescribeJob (GET /v20180820/jobs/{JobId}, 200, returns JobDescriptor). JobDescriptor.Status is enum com.amazonaws.s3control#JobStatus = Active|Cancelled|Cancelling|Complete|Completing|Failed|Failing|New|Paused|Pausing|Preparing|Ready|Suspended. Recorded behavior: with ConfirmationRequired=false the job walks New -> Preparing -> Active -> Complete without further input; S3 Batch Operations does not validate role assumability at CreateJob time but assumes RoleArn asynchronously once the job runs, so a job whose role is not yet assumable terminates Failed with a FailureReasons entry containing 'Could not assume the IAM role' — i.e. the terminal status itself is the observable. For Operation S3PutObjectTagging the tasks really run: every object named in the manifest ends up carrying exactly the job's TagSet (GetObjectTagging on the target returns it), and an enabled Report is written under <Prefix>/job-<JobId>/.

Emulator: the job never reaches any of Complete/Failed/Cancelled. The check's bounded DescribeJob poll (every 5 s, 90 s per attempt, 200 s overall retry deadline for the role-propagation retry loop) never observes a terminal status and the check is killed at the suite's 60 s wall clock, so neither the Complete assertion nor the GetObjectTagging assertion is ever reached. Nothing ahead of the poll is slow: s3.create_bucket, put_bucket_tagging, two put_objects, get_object_tagging, sts.get_caller_identity, iam.create_role + put_role_policy and s3control.create_job all answer immediately. Hypothesis: the job record is created and left in a non-terminal status with no engine advancing it (the same shape as the emulator's Glue job runs), rather than any single call hanging. An emulator that settles a job promptly completes this check well inside the budget, so re-replay it to confirm.

Why: Batch Operations is only meaningful as execution over accumulated object state. A client cannot wait for a job, cannot read a ProgressSummary, cannot consume a completion report, and — the specific point of this check — cannot verify that a role trusting batchoperations.s3.amazonaws.com is genuinely assumed and used to perform the action: the no-op job looks indistinguishable from one whose trust policy is wrong, so the deny path and the allow path both produce 'nothing happened'.

Fix: the s3control batch-job state machine (progress a submitted job through to a terminal status) and its task execution for S3PutObjectTagging, including the asynchronous role assumption that populates FailureReasons when the role cannot be assumed. The open issue 20261024T005340Z-s3control-public-access-block-not-enforced-getaccesspoint-getaccesspointpolicystatus.md records the same root cause as its Cause 7 (grounded in three s3control.* checks) and the open issue 20261024T190718Z-dynamodb-lambda-s3-secretsmanager-a-cloudformation-deployed-s3-batch-operations-job-with.md records the Lambda-invoke variant; neither carries this check's replay key, so it is recorded here and can be closed together with them — whoever fixes the job engine should re-replay this check id too.

Traces: emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive.

Service-attribution note: the leading token of the check id is the composite 'emr__glue__s3' (the check declares SERVICES = ['emr','glue','s3']), which is not a single service token. The failing operations are S3 Control's CreateJob/DescribeJob, so this issue is filed under service 's3control' (emr, glue, s3 and s3control are all bootstrapped).

## Evidence
Expected (AWS): s3control.create_job(AccountId, ConfirmationRequired=False, Operation={'S3PutObjectTagging': {'TagSet': [{'Key':'sim2real','Value':'tagged'}]}}, Manifest=<CSV with Bucket,Key fields>, Report={...Enabled: True}, Priority=1, RoleArn=<role trusting batchoperations.s3.amazonaws.com>) -> 200 + JobId; s3control.describe_job(AccountId, JobId)['Job']['Status'] reaches 'Complete' (or 'Failed' with a FailureReasons entry containing 'Could not assume the IAM role' while the role is still propagating); s3.get_object_tagging(Bucket, Key='data/target.txt')['TagSet'] == [{'Key':'sim2real','Value':'tagged'}].

Actual: 'Check timed out after 60.0s' on emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive. The check never left _run_job_until_role_assumed / _poll_until (describe_job every 5 s until Status in {Complete, Failed, Cancelled}); the assertions on final['Status'] == 'Complete' and on the target object's TagSet were never reached. The target object was verified untagged before the job (get_object_tagging -> []).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b); s3.put_object(Bucket=b, Key='data/target.txt', Body=b'hello'); etag = s3.put_object(Bucket=b, Key='manifest.csv', Body=b'<b>,data/target.txt\n')['ETag'].
3. iam.create_role with a trust policy for Principal {'Service': 'batchoperations.s3.amazonaws.com'} and an inline policy allowing s3:GetObject on arn:aws:s3:::<b>/*, s3:PutObject on arn:aws:s3:::<b>/reports/* and s3:PutObjectTagging on arn:aws:s3:::<b>/*.
4. s3control.create_job(AccountId=<account>, ConfirmationRequired=False, Operation={'S3PutObjectTagging': {'TagSet': [{'Key':'sim2real','Value':'tagged'}]}}, Report={'Bucket':'arn:aws:s3:::<b>','Prefix':'reports','Format':'Report_CSV_20180820','Enabled':True,'ReportScope':'AllTasks'}, Manifest={'Spec':{'Format':'S3BatchOperations_CSV_20180820','Fields':['Bucket','Key']},'Location':{'ObjectArn':'arn:aws:s3:::<b>/manifest.csv','ETag':etag}}, Priority=1, RoleArn=<role arn>, ClientRequestToken=<token>) -> 200 + JobId.
5. Poll s3control.describe_job(AccountId, JobId)['Job']['Status'] every 5 s: expect a terminal status (Complete, or Failed with 'Could not assume the IAM role'); observe it never becoming terminal.
6. s3.get_object_tagging(Bucket=b, Key='data/target.txt') -> expect [{'Key':'sim2real','Value':'tagged'}]; observe [].
7. Re-replay emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive.
