Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role cloudformation__iam__lambda__s3.cli-lambda-update-function-code-from-s3-positive
Issue-Key: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed|_cdk#17|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role|run#20|AssertionError|assert trust["Statement"][0]["Principal"] == {"AWS": f"arn:aws:iam::{acct}:root"}, (
Issue-Key: cloudformation__iam__lambda__s3.cli-lambda-update-function-code-from-s3-positive|_run_shell#15|AssertionError|raise AssertionError(
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# cloudformation__iam__lambda__s3: 3 net-new root causes — CreateChangeSet with TemplateURL is unimplemented (cdk deploy fails), trust-policy principals are not normalized to the account root ARN, and Lambda Invoke refuses every real invocation

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Three distinct net-new root causes, none marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 3: CreateChangeSet with TemplateURL (template fetched from S3) is a scaffold stub.
AWS: CreateChangeSet accepts either TemplateBody or TemplateURL; with TemplateURL CloudFormation fetches the template from S3 and creates the change set. This is the path the CDK always uses once assets are published, so `cdk deploy` depends on it.
Emulator: the call is refused with 'operation CreateChangeSet with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator', and `cdk deploy` exits rc=1 right after 'creating CloudFormation changeset...' (asset publishing to S3 succeeded first, so only the change-set step is missing).
Why: no CDK application can deploy against the emulator at all — the content-addressed asset flow gets as far as publishing and then dies, so every CDK-based workflow is untestable.
Fix: the CreateChangeSet handler in the cloudformation service crate must resolve TemplateURL by reading the object from the s3 service and then follow the TemplateBody path.
Traces: cloudformation__iam__lambda__s3.cdk-bootstrap-asset-deploy-is-content-addressed.

Cause 2 of 3: a trust policy's account-id principal is stored verbatim instead of being normalized to the account root ARN.
AWS: when a role's AssumeRolePolicyDocument names a principal as {"AWS": "<account-id>"}, IAM normalizes and returns it as {"AWS": "arn:aws:iam::<account-id>:root"}; readers of GetRole/the CDK's deploy-role PassRole check rely on seeing the root ARN.
Emulator: the deploy role's first trust principal reads back as {'AWS': '000000000000'} — the bare account id, un-normalized.
Why: tooling that compares a trust principal against the account root ARN (CDK's bootstrap/deploy-role validation, PassRole gating, policy diffing) sees a mismatch and refuses or mis-reports a correctly trusted role.
Fix: principal normalization when an AssumeRolePolicyDocument is stored/serialized, in the iam crate's CreateRole/UpdateAssumeRolePolicy handlers and the trust-document serializer.
Traces: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role.

Cause 3 of 3: Lambda Invoke refuses a real invocation of a deployed function with NotImplemented.
AWS: Invoke of an existing function with InvocationType=RequestResponse runs the deployment package and returns 200 with the function's payload.
Emulator: `NotImplemented` — 'Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator'. Here it arrives through the AWS CLI (`aws lambda invoke`), which exits rc=254.
Why: the positive leg of 'update function code from S3, then invoke to prove the new code runs' cannot complete, so a client cannot verify any code-update round trip.
Fix: the Invoke handler in the lambda service crate. The same root cause is already filed as 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md; the replay report did not attribute this check to it (the error surfaced through the CLI wrapper), so it is recorded here under this service token and should be closed with that work.
Traces: cloudformation__iam__lambda__s3.cli-lambda-update-function-code-from-s3-positive.

## Evidence
Cause 1: AssertionError: `cdk deploy` rc=1, expected 0: '... sim2real-cdkasset-...: creating CloudFormation changeset...\noperation CreateChangeSet with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator'.
Cause 2: AssertionError: deploy role's first trust principal was {'AWS': '000000000000'}, expected the account root (arn:aws:iam::000000000000:root).
Cause 3: AssertionError: Command failed (rc=254): cmd: ['aws','--output','json','lambda','invoke','--function-name','sim2real-codeupd-fn-a41f7706506d',...] stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: ... running its python3.12 deployment package is not implemented in this emulator.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: run `cdk deploy` of a stack with a bundled asset against the emulator endpoints (or call CreateChangeSet with TemplateURL pointing at an uploaded template object).
3. Cause 2: CreateRole with a trust document whose principal is {"AWS": "<account-id>"}, then GetRole and inspect the returned principal.
4. Cause 3: create a function, UpdateFunctionCode from S3, then `aws lambda invoke --function-name <fn>`.
5. Re-replay the three check ids in trace_ids to confirm.

## Resolution

All three causes of this report are resolved:

* Cause 1 — `CreateChangeSet` with `TemplateURL` resolves the object from this emulator's own
  Amazon S3 and follows the `TemplateBody` path (`ops/create_change_set.rs`,
  `state::template_url`). Replay: fixed.
* Cause 2 — a trust policy's account principal named by the account's id alone is stored and
  reported as that account's root-user ARN
  (`state::trust_policy::stored_trust_policy`, applied by `CreateRole`,
  `UpdateAssumeRolePolicy` and `AcquireRole`). Replay: the unit gets past that assertion.
* Cause 3 — `Invoke` runs a `python3.12` deployment package in process
  (`state::execution::python`). Replay: fixed.

One leg of the `cdk-deploy-role-passrole-rejects-non-cfn-exec-role` check is still open and is
NOT one of this report's causes: once the trust-principal assertion passes, that check goes on
to require a bucket create made with a narrowly-granted role session to be refused. Amazon S3
now authorizes a bucket's create and delete against the identity-based policies in force on the
caller (`ops/create_bucket.rs`, `ops/delete_bucket.rs`), but the recorded session is still
served, so the remaining divergence belongs to whichever caller class that gate deliberately
does not bound (`service/request_authorization`'s module docs name them). It needs a report of
its own.
