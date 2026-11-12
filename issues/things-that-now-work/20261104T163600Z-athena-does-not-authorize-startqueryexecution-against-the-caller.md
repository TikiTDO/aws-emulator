Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup|run#29|AssertionError|assert denied_code == "AccessDeniedException", (
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena does not authorize StartQueryExecution against the caller's workgroup-scoped IAM policy: a query on a non-granted workgroup succeeds instead of being denied

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Athena's resource-level authorization for workgroups is not applied. An identity whose IAM policy grants athena:StartQueryExecution only on one workgroup ARN can submit a query to a different workgroup and the call succeeds.

Cause 1 of 1: StartQueryExecution ignores the workgroup resource in the caller's IAM policy.
AWS: Athena authorizes StartQueryExecution against the workgroup resource `arn:aws:athena:<region>:<account>:workgroup/<WorkGroup>`. When the caller's identity policy allows athena:StartQueryExecution only on workgroup A, the identical request with only the WorkGroup member changed to B is refused with HTTP 400 AccessDeniedException; the same identity's call on A continues to succeed, so the refusal is scoped to the resource and not unconditional.
Emulator: the call on B succeeds — a QueryExecutionId is issued and no error is raised, so the resource-level condition is never evaluated.
Why: this is a permission-boundary failure that fails OPEN. A tenant that isolates teams or cost centres by granting each one its own Athena workgroup has no isolation at all against this emulator: a caller reaches every workgroup, including ones whose enforced result configuration points at another tenant's bucket. Any test or policy review validated against the emulator passes a policy that real AWS would deny — the divergence hides a privilege escalation rather than merely mis-reporting one.
Fix: the athena crate's StartQueryExecution handler must call the request authorizer with the workgroup ARN as the action's resource before the execution is created, and the emulator's IAM policy evaluation must honour resource-level matching for athena:StartQueryExecution.
Traces: athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup: the check creates two workgroups and an identity whose policy allows athena:StartQueryExecution on the first workgroup's ARN only, confirms the allowed workgroup works, then issues the identical StartQueryExecution with only WorkGroup changed to the non-granted name. Expected Error.Code='AccessDeniedException'; actual the call returned successfully — the harness records the sentinel 'UNEXPECTED_SUCCESS' for both code and message: 'the same identity making the same call with only the workgroup changed to <non-granted workgroup> must be refused with AccessDeniedException; got UNEXPECTED_SUCCESS'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect athena.workgroup-scoped-iam-policy-denies-start-query-execution-on-one-workgroup.
3. Minimal manual repro:
   a. CreateBucket B. CreateWorkGroup(Name=WG_YES) and CreateWorkGroup(Name=WG_NO), both with Configuration.ResultConfiguration.OutputLocation under B.
   b. Create an IAM principal whose policy allows athena:StartQueryExecution/GetQueryExecution on Resource='arn:aws:athena:<region>:<account>:workgroup/WG_YES' only (plus the S3 access it needs).
   c. As that principal, StartQueryExecution(QueryString='SELECT 1', WorkGroup=WG_YES) — expect success.
   d. As the same principal, StartQueryExecution(QueryString='SELECT 1', WorkGroup=WG_NO) — expect HTTP 400 AccessDeniedException; observe a successful response with a QueryExecutionId.
