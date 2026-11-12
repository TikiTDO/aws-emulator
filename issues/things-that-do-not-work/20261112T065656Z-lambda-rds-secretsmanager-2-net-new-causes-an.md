Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied
Issue-Key: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets|run#61|AssertionError|assert "FunctionError" not in resp, f"invoke failed: {resp.get('FunctionError')}"
Issue-Key: lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied|_confirm_target_never_available#30|AssertionError|raise AssertionError(
Issue-Commit: dedd6f8a44979b422a25c34412c44daa0d34e6e0

# lambda__rds__secretsmanager: 2 net-new causes — an in-function boto3 RDS Proxy read crashes the invocation, and a proxy target whose role cannot read its secret reaches AVAILABLE instead of UNAVAILABLE/AUTH_FAILURE

> _Note: this issue was identified against the code base at commit `dedd6f8a44979b422a25c34412c44daa0d34e6e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBProxyTargets

## Description
Two distinct root causes in the RDS Proxy / Secrets Manager auth surface, both reached from Lambda. They must both be closed before this issue is submitted.

=== Cause 1 of 2: a Lambda handler that reads an RDS Proxy with boto3 fails with an unhandled error ===
AWS: Smithy: rds#DescribeDBProxies returns DBProxies[], each carrying Auth[] whose entries hold SecretArn (the Secrets Manager secret the proxy authenticates with) for AuthScheme=SECRETS; rds#DescribeDBProxyTargets returns Targets[] (paginated by Marker/MaxRecords, errors DBProxyNotFoundFault, DBProxyTargetGroupNotFoundFault, DBProxyTargetNotFoundFault, InvalidDBProxyStateFault), each target carrying Type, RdsResourceId and TargetHealth{State, Reason, Description}. A function whose execution role grants rds:DescribeDBProxies and rds:DescribeDBProxyTargets therefore gets the same two documents the caller's own SDK gets, and `proxy['Auth'][0]['SecretArn']` and `targets[0]['TargetHealth']['State']` both resolve; Invoke answers 200 with no FunctionError header.
Emulator: the same two calls, made with boto3 *inside* the function, raise and the invocation is reported as `FunctionError: Unhandled` — even though the identical reads made from the harness moments earlier succeeded and the target had already been observed with Type=RDS_INSTANCE, RdsResourceId=<instance id> and TargetHealth.State=AVAILABLE. Replay detail: `AssertionError: invoke failed: Unhandled`. Hypothesis: the response the in-function SDK receives omits a member the handler dereferences (Auth[].SecretArn or Targets[].TargetHealth.State) or is not serialized in the query protocol shape boto3 parses, so the handler raises before returning.
Why: a client cannot read its own proxy wiring from inside a function. Any handler that resolves its proxy's secret or its target health — the documented way an application confirms it may connect through the proxy — dies with an opaque Unhandled error instead of reading the fields AWS serves, and Invoke reports a function fault where AWS reports success.
Fix: the rds DescribeDBProxies / DescribeDBProxyTargets serializers and the state they read from, so the Auth[].SecretArn and Targets[].TargetHealth members are present and wire-shaped identically for an in-function SDK caller as for an external one.
Traces: lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets.

=== Cause 2 of 2: a proxy whose role cannot read its secret still drives its target to AVAILABLE ===
AWS: Smithy: rds#TargetHealth has members State (enum TargetState: REGISTERING | AVAILABLE | UNAVAILABLE | UNUSED), Reason (enum TargetHealthReason: UNREACHABLE | CONNECTION_FAILED | AUTH_FAILURE | PENDING_PROXY_CAPACITY | INVALID_REPLICATION_STATE | PROMOTED) and Description ("If the State is AVAILABLE, a description is not included"); the documented lifecycle is registering > unavailable > available. An RDS Proxy configured with AuthScheme=SECRETS and a SecretArn, whose IAM role does NOT grant secretsmanager:GetSecretValue on that secret, cannot fetch the credentials, so the registered target never becomes healthy: DescribeDBProxyTargets reports TargetHealth.State=UNAVAILABLE with Reason=AUTH_FAILURE and a Description naming the credential failure, while an otherwise identical twin whose role does grant GetSecretValue reaches State=AVAILABLE; granting the permission afterwards recovers the first target. The requirement is the State/Reason pair, not any one run's description text.
Emulator: the target of the proxy whose role lacks secretsmanager:GetSecretValue reaches `AVAILABLE`. Replay detail: `AssertionError: target for <db instance> on proxy <proxy name> became AVAILABLE despite its role lacking secretsmanager:GetSecretValue` (the instance and proxy names are per-run values). TargetHealth is therefore reported from registration alone, with no dependency on the proxy role's ability to read the secret.
Why: the single most common RDS Proxy misconfiguration becomes invisible. A client that gates connection attempts on TargetHealth.State / Reason=AUTH_FAILURE sees a healthy target and proceeds, and a negative test that must distinguish a mis-permissioned proxy from a working one passes against a broken deployment.
Fix: the RDS Proxy target-health state machine — it must evaluate the proxy's RoleArn against secretsmanager:GetSecretValue for each Auth[].SecretArn before admitting a registered target, settling UNAVAILABLE with Reason=AUTH_FAILURE while the fetch is refused and transitioning to AVAILABLE once it is granted.
Traces: lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied.

(The one open issue in this group, 20261026T235825Z-lambda-rds-secretsmanager-an-rds-proxy-created-with.md, is about RequireTLS enforcement on the proxy endpoint and carries neither of these replay keys.)

## Evidence
Cause 1 — failing check [mismatch] lambda__rds__secretsmanager.cli-rds-describe-db-proxy-targets
Replay detail (verbatim, tail):
  File "check_lambda__rds__secretsmanager_cli_rds_describe_db_proxy_targets.py", line 241, in run
    assert "FunctionError" not in resp, f"invoke failed: {resp.get('FunctionError')}"
  AssertionError: invoke failed: Unhandled
Expected (AWS): Invoke -> 200 with no FunctionError; payload {'secret_arn': <the proxy's Auth[0].SecretArn>, 'target_state': 'AVAILABLE'}.
Actual (emulator): Invoke -> FunctionError 'Unhandled'; the handler's boto3 rds.describe_db_proxies(...)['DBProxies'][0]['Auth'][0]['SecretArn'] / rds.describe_db_proxy_targets(...)['Targets'][0]['TargetHealth']['State'] raised, although the same reads from outside the function had just returned Type=RDS_INSTANCE, RdsResourceId=<instance>, TargetHealth.State=AVAILABLE.

Cause 2 — failing check [mismatch] lambda__rds__secretsmanager.rds-proxy-auth-lambda-missing-permission-denied
Replay detail (verbatim, tail):
  File "check_lambda__rds__secretsmanager_rds_proxy_auth_lambda_missing_permission_denied.py", line 327, in _confirm_target_never_available
    raise AssertionError(
  AssertionError: target for sim2real-behav-dbi-70e71c89b300 on proxy sim2real-behav-proxy-bad-67c62f74569a became AVAILABLE despite its role lacking secretsmanager:GetSecretValue
Expected (AWS): DescribeDBProxyTargets -> Targets[0].TargetHealth.State = UNAVAILABLE with Reason = AUTH_FAILURE (enum TargetHealthReason) for the proxy whose role cannot GetSecretValue, and State = AVAILABLE for the granted twin; granting the permission recovers the first target.
Actual (emulator): Targets[0].TargetHealth.State = AVAILABLE for the ungranted proxy (the identifiers above are that run's generated names, not part of the requirement).

## Steps to Reproduce
Cause 1:
1. Create a MySQL db.t3.micro DB instance in a 2+ AZ subnet group and wait for status 'available'.
2. Create a Secrets Manager secret holding {username, password}; create an IAM role trusted by rds.amazonaws.com with secretsmanager:GetSecretValue on it.
3. CreateDBProxy (EngineFamily MYSQL, Auth [{AuthScheme: SECRETS, SecretArn: <secret>, IAMAuth: DISABLED}], RoleArn = that role) and wait for Status 'available'; RegisterDBProxyTargets with the DB instance and wait for TargetHealth.State = AVAILABLE.
4. Create a python3.12 Lambda whose role grants rds:DescribeDBProxies + rds:DescribeDBProxyTargets; the handler returns {'secret_arn': proxy['Auth'][0]['SecretArn'], 'target_state': targets[0]['TargetHealth']['State']}.
5. Invoke it. Expected: 200, no FunctionError. Observed: FunctionError 'Unhandled'.

Cause 2:
1. Create a MySQL DB instance and a Secrets Manager secret with its master credentials.
2. Create two rds.amazonaws.com-trusted roles: one WITH secretsmanager:GetSecretValue on that secret, one WITHOUT.
3. Create two proxies (AuthScheme SECRETS, same SecretArn), one per role, and RegisterDBProxyTargets with the DB instance on each.
4. Poll DescribeDBProxyTargets on both. Expected: the ungranted proxy's target stays UNAVAILABLE with TargetHealth.Reason = AUTH_FAILURE while the granted twin reaches AVAILABLE; then attach GetSecretValue to the first role and watch its target reach AVAILABLE.
Observed: the ungranted proxy's target reaches AVAILABLE.
