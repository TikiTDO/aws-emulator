Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.capability-access-denied eks.register-cluster-access-denied eks.register-cluster-client-request-token-is-idempotent eks.describe-cluster-versions-shape
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.capability-access-denied|run#18|AssertionError|assert codes == expected_codes, codes
Issue-Key: eks.register-cluster-access-denied|_expect_error#6|AssertionError|raise AssertionError("expected the call to fail, but it succeeded")
Issue-Key: eks.register-cluster-client-request-token-is-idempotent|_expect_error#11|AssertionError|raise AssertionError("call unexpectedly succeeded")
Issue-Key: eks.describe-cluster-versions-shape|_rejects_with#10|AssertionError|raise AssertionError(f"expected {expected_code!r} but the call succeeded")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# eks: 3 net-new over-permissive root causes — IAM authorization is never enforced, RegisterCluster accepts a duplicate cluster name, DescribeClusterVersions accepts invalid filters

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCluster

## Description
Three net-new root causes in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend) where the emulator ACCEPTS or mis-refuses what real EKS refuses. All three must be closed.

Cause 1 of 3 — eks operations never make an authorization decision: a principal without the permission is served (or gets a not-found) instead of AccessDeniedException.
AWS: Smithy: RegisterCluster models AccessDeniedException (httpError 403); the capability operations' denial is recorded behavior only. Recorded behavior: a caller whose role does not allow the action is refused with error code AccessDeniedException and the message "User: <principalArn> is not authorized to perform: eks:<Action> on resource: <resourceArn> because no identity-based policy allows the eks:<Action> action" — fixed text "User: ", " is not authorized to perform: eks:", " on resource: ", " because no identity-based policy allows the eks:", " action"; variable parts are the caller's assumed-role principal ARN, the operation name, and the resource ARN derived from the request (cluster ARN for CreateCapability/ListCapabilities/RegisterCluster/DeregisterCluster, capability ARN for Describe/DeleteCapability). Authorization precedes resource resolution: the denial is returned even when the cluster named in the request does not exist, and a denied RegisterCluster creates nothing.
Emulator: the restricted caller gets exactly what a fully privileged caller gets — ResourceNotFoundException ('The requested resource does not exist.' / 'No cluster found for name: <name>.') for the capability operations, and RegisterCluster/DeregisterCluster SUCCEED for a principal with no eks:RegisterCluster permission. The checks' admin-caller baseline leg passed, so the emulator reaches resource resolution for every caller and simply has no authorization step.
Why: every security-relevant test run against the emulator passes vacuously — least-privilege policies cannot be validated, and a client that expects a 403 (to re-auth or escalate) instead sees 404/200. This is over-permissive behavior, the worst class of fidelity gap.
Fix: an authorization step in the eks request pipeline (ahead of cluster/capability resolution) plus the AccessDeniedException message construction; at minimum for RegisterCluster, DeregisterCluster, CreateCapability, DescribeCapability, DeleteCapability and ListCapabilities.
Traces: eks.capability-access-denied, eks.register-cluster-access-denied.

Cause 2 of 3 — RegisterCluster does not enforce cluster-name uniqueness: a replay is accepted instead of refused.
AWS: Smithy: RegisterCluster models ResourceInUseException (httpError 409) and its clientRequestToken is an @idempotencyToken. Recorded behavior: the cluster NAME is the whole conflict key and the token participates in no deduplication — replaying RegisterCluster with the same name is refused ResourceInUseException with the message "Cluster already exists with name: <name>" (fixed text plus the name from the request) whether the clientRequestToken is identical, different or absent; conversely the same token under a different name mints a second, distinct cluster. Exactly one cluster carries the name afterwards.
Emulator: the byte-identical replay (same name, same clientRequestToken) SUCCEEDS — the check fails at `_expect_error(lambda: c.register_cluster(**body))` with 'call unexpectedly succeeded'.
Why: a client relying on ResourceInUseException to detect 'already registered' silently creates/overwrites state; an emulator-backed test cannot discover the name collision that real AWS reports, and no modeled 409 ever appears.
Fix: the RegisterCluster handler's conflict check and its cluster-name index in the eks crate's state (keyed on name, NOT on clientRequestToken).
Traces: eks.register-cluster-client-request-token-is-idempotent.

Cause 3 of 3 — DescribeClusterVersions accepts invalid clusterVersions / versionStatus filter values.
AWS: Smithy: DescribeClusterVersions.clusterVersions is a StringList @httpQuery and versionStatus targets the VersionStatus enum; the operation models InvalidParameterException (httpError 400). Recorded behavior: a clusterVersions entry that is not a Kubernetes version ('not-a-version') and a versionStatus outside the enum ('NOT_A_STATUS') are each refused with InvalidParameterException.
Emulator: both calls SUCCEED (the check's `_rejects_with(...)` reports 'expected InvalidParameterException but the call succeeded'). The happy-path catalog read in the same check passed, so this is the filter-validation path alone.
Why: a client typo in a version or status filter silently returns a catalogue page instead of a 400, so callers ship code that real AWS rejects.
Fix: the DescribeClusterVersions handler's query-parameter validation (enum membership for versionStatus, version syntax for clusterVersions).
Traces: eks.describe-cluster-versions-shape.

## Evidence
Suite simparity-behavioral-aws-v1: 64/310 passed; eks 60/282. None of these checks is marked ALREADY FILED or OWNED.

Cause 1: eks.capability-access-denied — AssertionError at `assert codes == expected_codes`: {'delete': ('ResourceNotFoundException', 'The requested resource does not exist.'), 'create': ('ResourceNotFoundException', 'No cluster found for name: sim2real-behav-cap-cl-156efedbe545.'), 'describe': ('ResourceNotFoundException', 'The requested resource does not exist.'), 'list': ('ResourceNotFoundException', 'No cluster found for name: sim2real-behav-cap-cl-156efedbe545.')} — expected each to be ('AccessDeniedException', 'User: arn:aws:sts::<acct>:assumed-role/<role>/sim2real-behav is not authorized to perform: eks:<Action> on resource: <cluster-or-capability-arn> because no identity-based policy allows the eks:<Action> action'). The admin baseline leg (the same four calls with full privileges) passed. eks.register-cluster-access-denied — AssertionError: expected the call to fail, but it succeeded (RegisterCluster/DeregisterCluster issued by a role without eks:RegisterCluster).
Cause 2: eks.register-cluster-client-request-token-is-idempotent — AssertionError: call unexpectedly succeeded at `replay_code, replay_msg = _expect_error(lambda: c.register_cluster(**body))`; expected ResourceInUseException 'Cluster already exists with name: <name>'.
Cause 3: eks.describe-cluster-versions-shape — AssertionError: expected 'InvalidParameterException' but the call succeeded, for describe_cluster_versions(clusterVersions=['not-a-version']).

## Steps to Reproduce
1. Boot the emulator. Create an IAM role whose only eks grant is eks:ListClusters and obtain credentials for it.
2. Cause 1: with that restricted client call list_capabilities / describe_capability / delete_capability / create_capability on any cluster name -> expect 403 AccessDeniedException 'User: <assumed-role arn> is not authorized to perform: eks:<Action> on resource: <arn> because no identity-based policy allows the eks:<Action> action'; observe ResourceNotFoundException. Then call register_cluster / deregister_cluster with the same restricted client -> expect the same denial; observe success.
3. Cause 2: register_cluster(name=N, connectorConfig={'roleArn': <connector role>, 'provider': 'OTHER'}, clientRequestToken=T) twice with the identical body -> the second call must be refused 409 ResourceInUseException 'Cluster already exists with name: N'; repeat with a fresh token and with no token at all (same refusal), and confirm list_clusters(include=['all']) reports N exactly once.
4. Cause 3: describe_cluster_versions(clusterVersions=['not-a-version']) and describe_cluster_versions(versionStatus='NOT_A_STATUS') -> each must be refused 400 InvalidParameterException.
