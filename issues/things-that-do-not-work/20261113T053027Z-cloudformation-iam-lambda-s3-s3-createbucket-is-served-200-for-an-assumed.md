Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role
Issue-Key: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role|_expect#21|AssertionError|raise AssertionError(f"{why}: the call succeeded but was expected to be refused")
Issue-Commit: 455edd8297574e8c9cd5843518c18f9edb691c55

# s3 CreateBucket is served 200 for an assumed-role session whose identity policies grant no s3:CreateBucket (implicit deny not enforced), so a CDK deploy-role chain cannot be probed

> _Note: this issue was identified against the code base at commit `455edd8297574e8c9cd5843518c18f9edb691c55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
This issue carries 1 root cause, in the s3 CreateBucket handler's authorization decision.

Cause 1 of 1: a bucket create made by an assumed-role session whose identity-based policies contain NO s3:CreateBucket statement is ACCEPTED and the bucket is created, instead of being refused 403 AccessDenied.

AWS: Smithy (com.amazonaws.s3) models CreateBucket as PUT /{Bucket} with only BucketAlreadyExists and BucketAlreadyOwnedByYou (both 409) as operation errors; the authorization refusal is S3's service-common 403 AccessDenied, so the model says nothing further and the rest is recorded behavior. Recorded behavior: a signed call by a principal that no identity-based policy allows the action to is refused with HTTP 403, error code AccessDenied and the message `User: <caller-arn> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no identity-based policy allows the s3:CreateBucket action`. Fixed text of that message: `User: `, ` is not authorized to perform: s3:CreateBucket on resource: `, ` because no identity-based policy allows the s3:CreateBucket action`, and the resource ARN is quoted. Variable parts, taken from the request and from state: the caller ARN (here the assumed-role session ARN `arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>`) and the bucket name from the request's Bucket label. No bucket comes into being, so a following HeadBucket by an authorized principal answers 404 Not Found. The emulator's own handler documents exactly this: services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket.rs lines 5-11 state the create is authorized before anything is written, under s3:CreateBucket, and that a principal no policy allows is refused 403 AccessDenied.

Emulator: the identical call on the session assumed from the CDK bootstrap `deploy` role — whose inline `default` policy the same check reads back and which holds only CloudFormation, S3-read, SSM and a single iam:PassRole scoped to the CFN exec role — SUCCEEDS. The replay reports `AssertionError: the deploy role holds no s3:CreateBucket, so creating 'sim2real-cdkchain-probe-...' is refused: the call succeeded but was expected to be refused`, i.e. a 200 with a created bucket where a 403 AccessDenied was due.

Why: a client relying on AWS's behavior gets no least-privilege boundary at all on bucket creation. Any role or session reachable in the account can create buckets, so deploy-role/exec-role separation (the whole point of the CDK role chain, and of any least-privilege review) cannot be demonstrated or tested: negative arms that must be refused silently pass, and the side effect (a real bucket) is left behind. Downstream, the probe-then-assert pattern every permissions test uses reports the opposite of the truth.

Fix: the CreateBucket handler's authorization leg in services/service-com-amazonaws-s3-amazons3/src/ops/create_bucket.rs together with the shared decision it defers to (crate::service::request_authorization) — the implicit deny must be applied to an assumed-role session principal whose in-force identity policies contain no matching s3:CreateBucket allow, and the refusal must carry 403/AccessDenied with the message above. Hypothesis (neither the model nor the recording states which): either that decision is never taken for this operation/principal kind, or the deploy role the emulator provisions for `cdk bootstrap` is granted S3 permissions beyond what the bootstrap template grants; the check's other arms read that role's inline policy back correctly (one iam:PassRole statement scoped to the CFN exec role), which points at the decision path rather than the provisioned policy.

Traces: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role (arm 5, the first negative arm; the check's later arms — iam:CreateRole refusal, the PassRole-scoping CreateStack refusal, and the control create with the CFN exec role — were never reached). No open issue carries this check: the suite report lists it unowned, and the neighbouring open report 20261111T093908Z-sts-a-getfederationtoken-session-policy-does-not-scope.md covers s3:CreateBucket served 200 for a GetFederationToken SESSION POLICY, which is a different principal and a different policy source.

## Evidence
Failing check: [mismatch] cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role

Replay detail (end):
  File "check_cloudformation__iam__lambda__s3_cdk_deploy_role_passrole_rejects_non_cfn_exec_role.py", line 150, in run
    _expect(lambda: deploy_s3.create_bucket(
  File "...", line 284, in _expect
    raise AssertionError(f"{why}: the call succeeded but was expected to be refused")
  AssertionError: the deploy role holds no s3:CreateBucket, so creating 'sim2real-cdkchain-probe-26106163f97f' is refused: the call succeeded but was expected to be refused

Expected (AWS): HTTP 403, Error.Code=AccessDenied, Error.Message=`User: arn:aws:sts::<account>:assumed-role/<deploy-role>/<session> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no identity-based policy allows the s3:CreateBucket action`, and no bucket created (a subsequent HeadBucket by an authorized principal answers 404 Not Found).
Actual (emulator): the CreateBucket call returned success and the bucket was created.

The session is assumed from the CDK bootstrap deploy role, whose inline policy the same check reads back in arm 3: exactly one iam:PassRole statement, scoped to the CFN execution role, plus CloudFormation/S3-read/SSM — no s3:CreateBucket.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a role trusting the account root (or run `cdk bootstrap` and use its `deploy` role) whose only inline permissions are cloudformation:*, s3 read-only, ssm:GetParameter and iam:PassRole scoped to one role ARN — no s3:CreateBucket.
3. sts.assume_role into it and build an s3 client from the returned credentials.
4. Call s3.create_bucket(Bucket='probe-<unique>', CreateBucketConfiguration={'LocationConstraint': <region>}).
5. Expect HTTP 403 AccessDenied with the message quoted above and no bucket; observe a successful create.
6. Re-replay the check id in trace_ids.
