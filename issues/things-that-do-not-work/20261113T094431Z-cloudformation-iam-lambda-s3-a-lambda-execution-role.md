Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative|_invoke_until#10|Timeout|raise AssertionError(
Issue-Commit: 0275b87193e8729c4e6e84626c64cbadccfa7c1a

# cloudformation__iam__lambda__s3: a Lambda execution role's permissions boundary is never enforced on the function's in-handler s3:PutObject, so the boundary-denial check never reaches a verdict

> _Note: this issue was identified against the code base at commit `0275b87193e8729c4e6e84626c64cbadccfa7c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause.

=== Cause 1 of 1: a permissions boundary attached to a Lambda execution role is not applied to the credentials the function runs with ===
AWS: recorded behaviour only (IAM permissions-boundary evaluation). An IAM role whose inline policy allows s3:* but whose attached permissions boundary allows only logs:CreateLogGroup / logs:CreateLogStream / logs:PutLogEvents has an effective permission set that is the intersection of the two, so a call the boundary does not allow is refused. A Lambda function assuming that role therefore gets AccessDenied for s3:PutObject with the message whose fixed text is "User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: s3:PutObject on resource: \"arn:aws:s3:::<bucket>/<key>\" because no permissions boundary allows the s3:PutObject action" — the account, role name, function name (the session name), bucket and key are taken from the request and the caller's session, and the "because no permissions boundary allows" clause is the reason this denial differs from a no-identity-policy denial. The object is not written (HeadObject answers 404). Removing the boundary makes the identical write succeed.
Emulator: the check's first observable step is a poll that re-invokes the function until its handler reports the PutObject failed; that condition is never reached and the check is cut off at its 60 s budget, with no refusal observed. The in-handler s3:PutObject is not refused, so neither the AccessDenied code, nor the "because no permissions boundary allows" message, nor the absent-object effect can be read back.
Why: a permissions boundary is the primary guardrail used to delegate role creation safely. If it is not applied to the credentials a Lambda function runs with, a function whose identity policy is broad can do anything the boundary was meant to forbid, and a client testing that guardrail is told nothing is wrong. The denial-reason clause also matters: an operator cannot tell a boundary denial from a missing-policy denial.
Fix: the authorization path that evaluates the Lambda execution role's effective permissions for in-function AWS calls — the permissions-boundary intersection in the IAM policy evaluator, and the AccessDenied message's "because no permissions boundary allows the <action> action" reason clause.
Traces: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative

Related open issue, which does not carry this check's replay key: 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md.

## Evidence
Error: Check timed out after 60.0s

The check (lambda-role-permission-boundary-restricts-s3-negative) creates a bucket and a Lambda execution role with an inline policy allowing s3:*, attaches a managed permissions boundary allowing only logs:CreateLogGroup/CreateLogStream/PutLogEvents via PutRolePermissionsBoundary, creates a python3.12 function on that role, and then polls:
  _invoke_until(ctx, fn, {"action": "put", ...}, lambda r: r.get("ok") is False, timeout_s=60)
waiting for the handler's boto3 s3.put_object to fail. That condition is never satisfied within the budget.

Expected, once the refusal arrives:
  error code 'AccessDenied'
  message 'User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/<key>" because no permissions boundary allows the s3:PutObject action'
  HeadObject on the key answers 404 (nothing written)
  after DeleteRolePermissionsBoundary, the identical write succeeds and the bytes round-trip.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create an S3 bucket and an IAM role trusted by lambda.amazonaws.com.
3. PutRolePolicy on the role allowing Action s3:* on Resource *.
4. CreatePolicy allowing only logs:CreateLogGroup/CreateLogStream/PutLogEvents on *, and PutRolePermissionsBoundary with it on the role.
5. CreateFunction (python3.12) on that role with a handler that calls s3.put_object and returns the error code it catches; wait for State Active.
6. Invoke the function with {"action": "put", "bucket": <bucket>, "key": "boundary.txt", "content": "should-not-land"} — the handler reports success instead of AccessDenied.
7. HeadObject on boundary.txt — the object exists instead of answering 404.
