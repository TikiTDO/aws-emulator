Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-proxy-cfn-routes-to-target rds.describe-db-proxy-targets-registration-order-preserved rds.e2e-proxy-target-becomes-routable rds.deregister-proxy-targets rds.deregister-proxy-target-not-registered-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-proxy-cfn-routes-to-target|run#30|TimeoutError|raise TimeoutError("proxy target AVAILABLE health: the run budget ended first; not graded")
Issue-Key: rds.describe-db-proxy-targets-registration-order-preserved|_wait_all_targets_present#29|TimeoutError|raise TimeoutError(f"proxy {proxy_name} listing all targets: the run budget ended first; not graded")
Issue-Key: rds.e2e-proxy-target-becomes-routable|run#37|AssertionError|assert matches, f"registered target {instance_id!r} missing from DescribeDBProxyTargets"
Issue-Key: rds.deregister-proxy-targets|_poll_until#20|Timeout|return last
Issue-Key: rds.deregister-proxy-target-not-registered-rejected|run#21|AssertionError|assert msg == "Missing targets", msg
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: DB proxy targets never become registrable or available — the proxy stays 'creating' and DescribeDBProxyTargets never reports a healthy target

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterDBProxyTargets

## Description
Two causes in the DB proxy target lifecycle.

Cause 1 of 2: a created DB proxy never leaves Status 'creating', so RegisterDBProxyTargets is permanently refused. AWS: CreateDBProxy returns DBProxy$Status 'creating' and the proxy settles to 'available' on its own; RegisterDBProxyTargets then returns the DBProxyTargets list, and InvalidDBProxyStateFault (Smithy: httpError 400, awsQueryError code InvalidDBProxyStateFault) is reserved for a proxy that is genuinely in a non-registrable state. Emulator: the proxy stays 'creating' for the whole run, so RegisterDBProxyTargets answers 400 InvalidDBProxyStateFault 'DB proxy <name> is in state creating and cannot take a target registration' forever — the registration can never succeed and the terminal state is unreachable. Checks: rds.deploy-rds-proxy-cfn-routes-to-target, rds.describe-db-proxy-targets-registration-order-preserved.

Cause 2 of 2: a target that WAS registered is not reported by DescribeDBProxyTargets and never reaches TargetHealth 'AVAILABLE'. AWS: after RegisterDBProxyTargets, DescribeDBProxyTargets lists the target with Type 'RDS_INSTANCE', its RdsResourceId and Endpoint, and TargetHealth$State settles to AVAILABLE; DeregisterDBProxyTargets then removes it and a deregistration of a target that is not registered is refused with DBProxyTargetNotFoundFault. Emulator: the registered instance is missing from DescribeDBProxyTargets, no target ever reports AVAILABLE, and the deregistration path cannot be exercised (the check reports the target 'is not registered with DB proxy <name>'). Checks: rds.e2e-proxy-target-becomes-routable, rds.deregister-proxy-targets, rds.deregister-proxy-target-not-registered-rejected.

Why: an RDS Proxy exists to front a database — with no registrable, discoverable, healthy target, no client can route through the proxy, no proxy-based connection pooling or IAM/secrets auth path can be exercised, and the negative contract (deregistering an unregistered target) is unreachable too. A proxy stuck in 'creating' also makes every caller's wait loop hang to its timeout rather than failing fast. Fix: the DB proxy state machine (creating→available) and the target-group/target registry in the rds crate's proxy state plus describe_db_proxy_targets.rs / register_db_proxy_targets.rs / deregister_db_proxy_targets.rs. Related open issue 20261030T001542Z-rds-deletedbproxy-never-removes-the-proxy-describedbproxies-keeps.md concerns proxy deletion, not target registration.

## Evidence
Expected: DescribeDBProxies reports Status 'available' within the create window; RegisterDBProxyTargets answers 200 with DBProxyTargets; DescribeDBProxyTargets lists the target and TargetHealth.State becomes 'AVAILABLE'.
Actual: 'TimeoutError: proxy target AVAILABLE health: the run budget ended first', 'TimeoutError: proxy <name> listing all targets: the run budget ended first', "AssertionError: registered target '<db id>' missing from DescribeDBProxyTargets", 'AssertionError: <cluster id> is not registered with DB proxy <proxy>', and (from the sibling lambda suite, same proxy state machine) 'botocore.errorfactory.InvalidDBProxyStateFault: An error occurred (InvalidDBProxyStateFault) when calling the RegisterDBProxyTargets operation: DB proxy <name> is in state creating and cannot take a target registration'.

## Steps to Reproduce
1. create_db_proxy(DBProxyName=<p>, EngineFamily='MYSQL', Auth=[...], RoleArn=<role>, VpcSubnetIds=[...]).
2. Poll describe_db_proxies(DBProxyName=<p>) — Status never leaves 'creating'.
3. register_db_proxy_targets(DBProxyName=<p>, DBInstanceIdentifiers=[<available instance>]) — 400 InvalidDBProxyStateFault; where the registration does return, describe_db_proxy_targets(DBProxyName=<p>) never lists the target and no TargetHealth.State reaches 'AVAILABLE'.
4. deregister_db_proxy_targets for a target that was never registered — the modeled not-found refusal is unreachable.
