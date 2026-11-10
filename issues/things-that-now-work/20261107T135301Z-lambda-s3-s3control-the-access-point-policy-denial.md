Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly
Issue-Key: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly|_invoke_payload#16|Timeout|return json.loads(body)
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# lambda__s3__s3control: the access-point-policy denial contrast never settles

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccessPointPolicy

## Description
One net-new root cause under '### lambda__s3__s3control'; this check is not marked ALREADY FILED or OWNED and it is the only failing check in that service.

Cause 1 of 1: an access point whose policy denies the Lambda role never produces the observable contrast AWS gives.
AWS: an S3 access point policy that denies a role is enforced on requests made through the access point ARN (403 AccessDenied), while the same role's direct bucket request, allowed by the bucket policy and its identity policy, still succeeds; both legs answer in one round trip each.
Emulator: the check ends with 'Error: Check timed out after 60.0s' — neither leg reaches an observable result inside the budget.
Why: a client cannot verify that access-point policies are evaluated separately from the bucket policy, so an access point that should deny may be silently permissive.
Fix: access-point policy evaluation in the s3control/s3 request authorizer, and whatever leaves the request unanswered.
Traces: lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly.

## Evidence
'Error: Check timed out after 60.0s' for lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly; no status or error code was produced for either the access-point leg or the direct-bucket leg.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite.
2. Re-replay lambda__s3__s3control.xsvc-s3-3-access-point-policy-denied-lambda-role-still-reads-bucket-directly (listed under '### lambda__s3__s3control').
3. CreateAccessPoint on a bucket, PutAccessPointPolicy denying the Lambda execution role, then have the function GetObject through the access point ARN and directly through the bucket.
