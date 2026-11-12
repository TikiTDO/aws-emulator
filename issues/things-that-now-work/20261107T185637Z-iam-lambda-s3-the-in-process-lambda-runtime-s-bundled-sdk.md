Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected
Issue-Key: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# The in-process Lambda runtime's bundled SDK has no s3 client, so a bucket-policy principal check cannot be exercised

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261105T085348Z-iam-lambda-s3-2-net-new-root-causes.md owns another check of this service and does not carry this replay key.

Cause 1 of 1: function code cannot construct an S3 client, so the bucket policy under test is never evaluated.
AWS: the managed python runtime's boto3 builds a client for any service; a handler calling s3.get_object reaches S3, which evaluates the bucket policy. A bucket policy naming a live role principal authorizes the read (200 with the body); once that role is deleted, S3 rewrites the principal to the role's unique id and the read is refused 403 AccessDenied.
Emulator: the handler faults before the call. The check polls 120 s for the authorized read and reports "the live-principal bucket policy authorizes the Lambda read not met within 120.0s. Last result: {'code': 'UnknownServiceError'}" — the bundled SDK's client factory refuses 's3' (its registry lists only sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager).
Why: the positive control can never pass, so the deleted-principal refusal it gates is unfalsifiable; more generally no function in the emulator can call S3, and an IAM/bucket-policy outcome is indistinguishable from this SDK fault.
Fix: the in-process Lambda runtime's bundled AWS SDK shim — its service registry and the client factory that refuses unknown services.
Traces: iam__lambda__s3.bucket-policy-deleted-role-principal-rejected.

## Evidence
Expected the handler's GetObject to be authorized by the live-principal bucket policy (then refused 403 AccessDenied after the role is deleted). Actual: {'code': 'UnknownServiceError'} on every attempt for 120s — the client for 's3' could not be built inside the runtime.

## Steps to Reproduce
1) Create a role and a bucket policy naming that role's ARN as Principal with s3:GetObject.
2) Create a function with that role whose handler calls boto3.client('s3').get_object and Invoke it.
3) Observe the UnknownServiceError payload instead of the object body.
