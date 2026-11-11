Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative|_invoke_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__iam__lambda__s3: the permission-boundary leg never completes — lambda-role-permission-boundary-restricts-s3-negative times out with no response

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the deploy-then-invoke flow that proves an IAM permissions boundary narrows a Lambda execution role's S3 access makes no observable progress, so the negative case is never reached.
AWS: a Lambda function whose execution role carries a PermissionsBoundary is invocable, and an in-function S3 call is authorized against the INTERSECTION of the role's identity policy and the boundary — an action the identity policy allows but the boundary does not is refused with 403 AccessDenied and the implicit-deny message `User: <assumed-role session ARN of the execution role> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action` (fixed text; principal, action and resource come from the request and the function's role). The function returns, so the caller observes that refusal in the invocation result.
Emulator: the check is reported `[mismatch] ... Error: Check timed out after 60.0s` — the flow produces nothing to assert against inside its budget, so neither the invocation nor the boundary-narrowed refusal is observable. Eleven other checks in this same section are owned by issues about this surface, and the closest is 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md (the Lambda provisioner and in-process runtime), whose unfinished function provisioning/invocation is the plain reading of this timeout; that issue does not carry THIS check id, so the permissions-boundary intersection behaviour it exercises is not pinned by anything filed.
Why: a permissions boundary is the standard delegation guard-rail — a client that relies on it to cap what a function's role can do cannot verify that the cap is enforced, and an unanswered invocation gives it no error to branch on at all (it is indistinguishable from a hung network, so retry and timeout logic misbehaves).
Fix: the Lambda provisioner/in-process runtime path that makes a CloudFormation-deployed function invocable, and the IAM authorization applied to the function's in-handler AWS calls — the boundary must be intersected with the role's identity policy and the resulting refusal returned to the caller, rather than the invocation never completing.
Traces: cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudformation__iam__lambda__s3` section (19/36 passed).

[mismatch] cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative
  Error: Check timed out after 60.0s

The check (GROUP = 'permission-boundary', SERVICES = cloudformation, iam, lambda, s3) builds a function zip in-process, deploys it with an execution role carrying a PermissionsBoundary, and invokes it to observe the boundary-narrowed S3 refusal; it reaches no assertion. Related owned sibling in the same section (corroboration only, not re-filed): cfn-iam-role-permissions-boundary-in-template-positive — OWNED by 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Deploy a stack with an S3 bucket, an IAM managed policy used as a permissions boundary that does NOT allow the S3 action under test, and a Lambda function whose execution role has a broad S3 identity policy plus that PermissionsBoundary.
3. Invoke the function and have it perform the S3 action.
4. AWS returns the function's result containing a 403 AccessDenied implicit-deny refusal for that action; the emulator produces no result within the check's budget.
5. Replay cloudformation__iam__lambda__s3.lambda-role-permission-boundary-restricts-s3-negative; it ends 'Check timed out after 60.0s'.
