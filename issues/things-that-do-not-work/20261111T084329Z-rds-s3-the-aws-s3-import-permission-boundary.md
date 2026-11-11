Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-s3-import-role-missing-s3-getobject-negative
Issue-Key: rds__s3.rds-s3-import-role-missing-s3-getobject-negative|_run_import_with_retry#20|Timeout|time.sleep(interval_s)
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# rds__s3: the aws_s3 import permission-boundary flow never converges, so an s3Import role without s3:GetObject is never observed failing the import

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBInstance

## Description
1 net-new root cause, of the same non-convergence family as the rds-only timeouts but in the rds+s3 integration path, and not covered by any ALREADY FILED or OWNED marker.

Cause 1 of 1: the s3Import role / aws_s3.table_import_from_s3 permission-boundary flow never reaches a terminal state.
AWS: an IAM role that trusts rds.amazonaws.com and is associated to a PostgreSQL DB instance with FeatureName `s3Import` reaches an `ACTIVE` association status (DescribeDBInstances AssociatedRoles[].Status) regardless of what the role's permission policy allows; calling aws_s3.table_import_from_s3() through it then FAILS with an S3 access error raised by the engine and writes no row, while re-associating a role that does grant s3:GetObject and s3:ListBucket on the same object makes the identical import succeed. The refusal is produced by the engine at import time, not by the association.
Emulator: the flow — create bucket and object, create the role, create the postgres instance, wait for `available`, AddRoleToDBInstance, wait for the association to read `ACTIVE`, then run the import — never completes; the check is killed at its 60 s budget. No HTTP error and no SQL error is produced, so neither the ACTIVE association nor the import refusal is ever observed.
Why: a client cannot establish that RDS-to-S3 authorization is enforced at import time. The flow that MUST fail (role without s3:GetObject) and the flow that MUST succeed (role with it) are indistinguishable here, so a deployment whose S3 import permissions are wrong gets no signal — the permission boundary is unverifiable rather than merely mis-reported.
Fix: the DB-instance status settling and the IAM-role-association status schedule (AddRoleToDBInstance → AssociatedRoles[].Status ACTIVE) in the rds state, plus the aws_s3 extension's authorization check against the associated role on the emulated PostgreSQL engine.
Traces: rds__s3.rds-s3-import-role-missing-s3-getobject-negative.

## Evidence
Error: Check timed out after 60.0s — no HTTP status, no error code, no SQL error. The emulator answered every control-plane call, but the chain never reached the state the check polls for.

Expected (AWS): DescribeDBInstances reports the s3Import role association with Status `ACTIVE`; `SELECT aws_s3.table_import_from_s3(...)` through the role that lacks s3:GetObject raises an S3 access error from the engine and the target table stays empty; after associating a role granting s3:GetObject and s3:ListBucket on the same bucket/key the identical import succeeds and the rows land.

Actual (emulator): the flow does not get through its waits inside the 60 s budget, so no association status and no import outcome — refusal or success — is ever observed.

## Steps to Reproduce
1. CreateBucket and PutObject a small CSV (e.g. key `sim2real-import.csv`, body `id,name\n1,alpha\n2,beta\n`).
2. Create an IAM role trusting rds.amazonaws.com whose policy grants NO s3:GetObject on that bucket.
3. CreateDBInstance(Engine=postgres, db.t3.micro) and poll DescribeDBInstances to `available`.
4. AddRoleToDBInstance(DBInstanceIdentifier=<id>, RoleArn=<role>, FeatureName='s3Import') and poll DescribeDBInstances until AssociatedRoles[0].Status == 'ACTIVE'.
5. Connect to the instance endpoint with psycopg, CREATE EXTENSION aws_s3, and run SELECT aws_s3.table_import_from_s3(...) — expect an S3 access error and zero rows.
6. Associate a role that does grant s3:GetObject/s3:ListBucket on the same object and re-run the identical import — expect success.
The run is killed at the 60 s per-check budget before step 4 or 5 is reached.
