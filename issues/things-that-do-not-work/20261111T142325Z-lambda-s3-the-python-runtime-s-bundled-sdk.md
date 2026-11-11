Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.in-handler-boto3-reports-its-version-and-lists-buckets
Issue-Key: lambda__s3.in-handler-boto3-reports-its-version-and-lists-buckets|run#13|AssertionError|assert error is None, f"the handler failed with FunctionError={error}: {json.dumps(got)[:500]}"
Issue-Commit: 53bfbc08a5ec2983a62510ba6bf3546a998019d9

# lambda__s3: the Python runtime's bundled SDK binds no S3 ListBuckets/GetBucketLocation, so an in-handler boto3 S3 bucket call raises SdkCallNotMade instead of being sent

> _Note: this issue was identified against the code base at commit `53bfbc08a5ec2983a62510ba6bf3546a998019d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: the AWS SDK the emulator's python3.12 Lambda runtime supplies to a handler binds only a fixed allowlist of S3 operations; ListBuckets (and GetBucketLocation) are not among them, so the call never leaves the function and the handler faults.

AWS: recorded behavior only for the runtime's contents. A Lambda python3.12 function's bundled boto3 binds the full S3 API, so `boto3.client('s3').list_buckets()` reaches S3 and answers HTTP 200 with Buckets (including the caller's own bucket) and an Owner.ID, and `get_bucket_location(Bucket=...)` answers the bucket's LocationConstraint. boto3.__version__ and botocore.__version__ are 1.x.y strings.

Emulator: Invoke answers FunctionError 'Unhandled' with errorType 'SdkCallNotMade' and errorMessage 'the AWS SDK this runtime supplies binds no ListBuckets call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2, GetBucketNotificationConfiguration, PutBucketNotificationConfiguration, GetBucketTagging, PutBucketTagging, ListObjectVersions, DeleteObjects'. The operation name in that sentence and the allowlist it recites are the runtime's own values; the requirement is that the call be dispatched to S3 at all. GetBucketLocation is likewise absent from the allowlist, so the handler cannot reach its second assertion either.

Why: a client relying on AWS's behavior sees its own handler code blamed for a call the runtime refused to make. Any handler that enumerates buckets or resolves a bucket's region — a very common bootstrap/diagnostic pattern — cannot run, and the failure surfaces as an unhandled application error rather than as a missing runtime capability.

Fix: the Lambda Python runtime's bundled SDK client — bind the S3 operations the S3 service implements rather than a fixed operation allowlist (and fail any genuinely unsupported call as a refusal that names the runtime, not the handler).

Related open issue 20261111T040849Z-lambda-s3-the-lambda-python-runtime-s-bundled-aws-sdk.md describes this same allowlist (its Cause 1) but does not carry this check's replay key; the allowlist recited by this run is already longer than the one recorded there, so the gap is narrowing but ListBuckets and GetBucketLocation are still unbound.

## Evidence
lambda__s3.in-handler-boto3-reports-its-version-and-lists-buckets [mismatch]:
AssertionError: the handler failed with FunctionError=Unhandled: {"errorMessage": "the AWS SDK this runtime supplies binds no ListBuckets call on Amazon S3; the operations it binds are GetObject, HeadObject, PutObject, CopyObject, DeleteObject, ListObjectsV2, GetBucketNotificationConfiguration, PutBucketNotificationConfiguration, GetBucketTagging, PutBucketTagging, ListObjectVersions, DeleteObjects", "errorType": "SdkCallNotMade", "requestId": "<per-run id>", "stackTrace": ["  File \"/var/task/index.py\", line 7, in handler\n    listed ..."]}

Expected: no FunctionError; the handler returns boto3/botocore 1.x.y version strings, list_status 200, listed_own_bucket true, owner_id_present true, and location equal to the bucket's region.

## Steps to Reproduce
1. CreateBucket a bucket (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. CreateFunction with Runtime 'python3.12' and a handler that does `s3 = boto3.client('s3'); listed = s3.list_buckets(); location = s3.get_bucket_location(Bucket=event['bucket'])` and returns boto3.__version__, botocore.__version__, the list status, whether its own bucket is listed, whether Owner.ID is present, and the LocationConstraint. Grant the role s3:ListAllMyBuckets and s3:GetBucketLocation.
3. Invoke it with {"bucket": "<bucket>"} — the invoke answers FunctionError 'Unhandled' with errorType 'SdkCallNotMade' naming ListBuckets, instead of the handler's result.
