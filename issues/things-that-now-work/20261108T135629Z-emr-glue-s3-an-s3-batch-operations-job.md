Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected
Issue-Key: emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected|run#15|AssertionError|assert bad_final["Status"] == "Failed", f"expected Failed, got: {bad_final}"
Issue-Commit: 9e2a1177787692948f5e67ac213048cdaf07cba6

# emr__glue__s3: an S3 Batch Operations job whose role does not trust batchoperations.s3.amazonaws.com runs to Complete and tags its target instead of settling Failed

> _Note: this issue was identified against the code base at commit `9e2a1177787692948f5e67ac213048cdaf07cba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
One root cause.

Cause 1 of 1: the S3 Control Batch Operations job runner never assumes the job's RoleArn, so a role that does not trust the Batch Operations service principal still gets its tasks executed.
AWS: s3control CreateJob accepts a job without synchronously validating the RoleArn's trust policy (role validation is asynchronous). When the job starts running, Batch Operations assumes that role; if the role's AssumeRolePolicyDocument does not allow batchoperations.s3.amazonaws.com to assume it, the role assumption fails and the job settles with Status 'Failed' carrying a non-empty FailureReasons whose FailureReason names the assume-role failure (the recorded text contains "Could not assume the IAM role"; the role ARN is the variable part), and none of the job's targets are modified — the manifest's object is left untagged. An otherwise identical job whose role trusts batchoperations.s3.amazonaws.com settles Complete and tags its target, which isolates the difference to the trust principal. Smithy (s3control): DescribeJob returns JobDescriptor with Status (JobStatus, whose values include Failed and Complete), FailureReasons (list of JobFailure{FailureCode, FailureReason}) and ProgressSummary; the model does not state the role-assumption rule, so the Failed verdict and the reason text are recorded behavior only.
Emulator: the mistrusted job reaches Status 'Complete' with ProgressSummary {TotalNumberOfTasks: 1, NumberOfTasksSucceeded: 1, NumberOfTasksFailed: 0}, no FailureReasons member at all, a TerminationDate three seconds after CreationTime, and the S3PutObjectTagging operation IS applied — the target object comes back carrying the job's tag. The job's RoleArn is stored and echoed but never assumed, and the role's trust policy is never evaluated.
Why: a client that uses Batch Operations to apply bulk mutations relies on the job status to tell it whether the work was authorized. Here a job backed by a role the service cannot assume reports complete success, so the client believes a privileged bulk mutation was performed under a role that AWS would have refused — and a mis-scoped or mistyped trust policy, the single commonest Batch Operations setup error, is invisible until the same job is run against real AWS, where it fails and nothing is applied. The converse is worse: the tasks really do run, so objects are mutated under an identity that was never authorized.
Fix: the s3control Batch Operations job runner — before executing a job's tasks it must assume the job's RoleArn, evaluating that role's trust policy for the batchoperations.s3.amazonaws.com service principal, and on failure settle the job Failed and populate FailureReasons with AWS's assume-role FailureReason instead of running the operation. The related open issues 20261107T182201Z-emr-glue-s3-2-net-new-root-causes-f0288f.md (a Batch Operations job never leaving 'Ready') and 20261102T121736Z-s3control-s3-control-batch-job-s3putobjecttagging-never-reaches-a.md (the job never reaching a terminal status) describe earlier states of this runner and do not cover this check: the job now does reach a terminal status, and the defect is that the terminal status is the wrong one.
Traces: emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected

## Evidence
emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected (mismatch)
AssertionError: expected Failed, got: {'JobId': '…', 'ConfirmationRequired': False, 'Description': 'sim2real-behav-negative-bad', 'JobArn': 'arn:aws:s3:<region>:<account>:job/…', 'Status': 'Complete', 'Manifest': {'Spec': {'Format': 'S3BatchOperations_CSV_20180820', 'Fields': ['Bucket','Key']}, 'Location': {…}}, 'Operation': {'S3PutObjectTagging': {'TagSet': [{'Key': 'sim2real', 'Value': 'tagged'}]}}, 'Priority': 1, 'ProgressSummary': {'TotalNumberOfTasks': 1, 'NumberOfTasksSucceeded': 1, 'NumberOfTasksFailed': 0, …}, 'Report': {…}, 'RoleArn': 'arn:aws:iam::<account>:role/…batchbadtrust…'}

Expected (AWS): Status 'Failed', a non-empty FailureReasons containing a FailureReason that names the assume-role failure ("Could not assume the IAM role …"), and GetObjectTagging on the manifest's target returning a TagSet WITHOUT {'Key':'sim2real','Value':'tagged'}.
Actual (emulator): Status 'Complete', FailureReasons absent, NumberOfTasksSucceeded 1, and the target object tagged.
Positive control in the same check (identical job whose role trusts batchoperations.s3.amazonaws.com) passed: Status 'Complete' and the target tagged — so the emulator produces the same outcome for both trust policies.

## Steps to Reproduce
1. CreateBucket and PutObject a one-line CSV manifest (Bucket,Key) plus the target object.
2. iam CreateRole twice with the SAME inline permissions policy (s3:GetObject, s3:PutObject on reports/*, s3:PutObjectTagging) but different trust principals: one trusting Service batchoperations.s3.amazonaws.com, the other trusting Service ec2.amazonaws.com.
3. s3control CreateJob for each role: Operation S3PutObjectTagging with TagSet [{Key:'sim2real',Value:'tagged'}], the CSV manifest, Priority 1, ConfirmationRequired false, a Report under reports/<label>.
4. Poll s3control DescribeJob until Status is Complete/Failed/Cancelled.
5. The ec2.amazonaws.com-trusting job must be Failed with FailureReasons naming the assume-role failure, and s3 GetObjectTagging on its target must NOT contain the tag; the emulator reports Complete with the tag applied.
