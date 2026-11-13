Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.auth-token-dual-validity-window elasticache.auth-token-special-characters-preserved elasticache.cache-cluster-resp-protocol-round-trip-over-socket elasticache.e2e-cluster-scale-failover-data-consequence elasticache.e2e-cluster-snapshot-restore-data-consistency elasticache.e2e-cluster-snapshot-restore-data-persists elasticache.e2e-serverless-snapshot-restore-flow elasticache.e2e-user-group-access-revocation-flow elasticache.e2e-user-group-auth-enforcement-flow elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure elasticache.elasticache-redis-set-get elasticache.elasticache-redis-key-expiry-removes-value-and-reports-ttl elasticache.elasticache-iam-auth-token-opens-a-session-on-a-serverless-cache elasticache.shard-configuration-resize-transitions elasticache.delete-primary-cluster-automatic-failover-rejected elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.auth-token-dual-validity-window|_probe_call#18|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.auth-token-special-characters-preserved|_probe_call#14|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.cache-cluster-resp-protocol-round-trip-over-socket|_probe_call#24|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-cluster-scale-failover-data-consequence|_probe_call#13|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-cluster-snapshot-restore-data-consistency|_probe_call#15|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-cluster-snapshot-restore-data-persists|_probe_call#16|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-serverless-snapshot-restore-flow|_probe_call#12|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-user-group-access-revocation-flow|_probe_call#25|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.e2e-user-group-auth-enforcement-flow|_probe_call#23|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments|_resolved_endpoint#9|PublishedUrlRejected|return ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure|_resolved_endpoint#17|PublishedUrlRejected|return ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.elasticache-redis-set-get|_probe_call#20|PublishedUrlRejected|host, port = ctx.endpoint_host(host, int(port))
Issue-Key: elasticache.elasticache-redis-key-expiry-removes-value-and-reports-ttl|_invoke#2|AssertionError|raise _no_executor(
Issue-Key: elasticache|CreateUser|DuplicateUserName
Issue-Key: elasticache|CreateReplicationGroup|InvalidParameterCombination
Issue-Key: elasticache.delete-primary-cluster-automatic-failover-rejected|_error#13|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected|_assert_error#19|AssertionError|assert _fault_base(code) == _fault_base(expected), (
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# ElastiCache serves endpoints it does not listen on, refuses valid input and permits deletes AWS refuses (6 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Six net-new root causes in the elasticache emulator, each grounded in failing behavioral checks the suite did NOT mark ALREADY FILED or OWNED. They are semantic defects (an unreachable published endpoint, refusals of valid input, a missing refusal, a wrong error code), not message-text defects; the message-only divergences of this service are filed separately.

Cause 1 of 6: every cache endpoint the control plane publishes is a synthesized real-AWS DNS name that the emulator does not serve, so no client can reach the cache it was just handed.
  AWS: recorded behavior only for the data plane (the Smithy model describes the control plane). In AWS, DescribeCacheClusters/DescribeReplicationGroups/DescribeServerlessCaches hand back an Endpoint {Address, Port} that resolves and accepts a RESP (or TLS+RESP) connection from inside the cache's VPC once Status is 'available'; an endpoint the service advertises is an address the service answers on. An emulator must therefore publish an address a customer of THIS emulator can connect to (its own endpoint host, a documented own host, or a loopback address for a loopback endpoint).
  Emulator: the endpoint serializer mints AWS-production hostnames and publishes them verbatim — node endpoints of the form `<cluster-id>-<node>.<nnnnnn>.<region>.cache.amazonaws.com:<port>`, replication-group primaries `master.<replication-group-id>.<nnnnnn>.<region>.cache.amazonaws.com:<port>`, and serverless caches `<name>-<nnnnnn>.serverless.<region>.cache.amazonaws.com:<port>`. The harness refuses the value the emulator published: `simparity.published.PublishedUrlRejected: endpoint_host was given <that host>:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts`. The fixed part of that divergence is the shape of the announced host (an amazonaws.com name the emulator never answers on); the ids, index digits and region in it are per-run values.
  Why: every real client connects to the advertised address. Against this emulator the control plane reports success and returns an address that resolves to AWS (or nowhere), so the application's first connect leaves the emulator entirely or fails; no cache semantics (SET/GET, TTL, INCR, eviction, AUTH, RBAC, restore-from-snapshot) can be exercised at all, and 12 data-plane checks measure nothing.
  Fix: the endpoint construction/serialization shared by the cache-cluster, replication-group and serverless-cache handlers and their state (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/*.rs plus the state that stores node/primary/serverless endpoints) must publish only an address the emulator's own RESP listener is bound to. Related open issues 20261109T010905Z-elasticache-deletereplicationgroup-never-applies-the-snapshot-in-progress.md (cause 2) and 20261108T152921Z-elasticache-in-vpc-data-plane-unverifiable-the-emulator.md describe the same family from the probe side (nothing answers / the in-VPC probe cannot run); neither states that the published host is not one of the emulator's own hosts, which is what this cause requires.
  Traces: elasticache.auth-token-dual-validity-window, elasticache.auth-token-special-characters-preserved, elasticache.cache-cluster-resp-protocol-round-trip-over-socket, elasticache.e2e-cluster-scale-failover-data-consequence, elasticache.e2e-cluster-snapshot-restore-data-consistency, elasticache.e2e-cluster-snapshot-restore-data-persists, elasticache.e2e-serverless-snapshot-restore-flow, elasticache.e2e-user-group-access-revocation-flow, elasticache.e2e-user-group-auth-enforcement-flow, elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments, elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure, elasticache.elasticache-redis-set-get.

Cause 2 of 6: the emulator's Lambda Python runtime has no `_ssl` module, so the in-VPC RESP probe this service's data-plane checks deploy cannot even import its handler.
  AWS: recorded behavior only. On AWS a python3.12 Lambda can `import ssl`; a VPC-attached function invoked with Invoke returns StatusCode 200 and no FunctionError, and the handler can open a TLS RESP session to the cache endpoint.
  Emulator: Invoke of the probe function returns StatusCode=200 with FunctionError='Unhandled' and payload `{"errorMessage":"Unable to import module 'index': No module named '_ssl'","errorType":"Runtime.ImportModuleError"}`. The fixed text is the missing-module error for `_ssl`; the function name in the surrounding report line is a per-run value.
  Why: a customer cannot run any TLS-using code (every AWS SDK over HTTPS, every in-VPC cache client) on this emulator's Lambda runtime, and this check can observe nothing about key expiry/TTL.
  Fix: the Lambda runtime image/python build the emulator ships for in-VPC functions (it must provide the `_ssl` extension module). Related open issues 20261108T152921Z-elasticache-in-vpc-data-plane-unverifiable-the-emulator.md and 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md record this runtime gap for OTHER checks (and 20261105T090541Z names `register_at_fork`, a different missing piece); neither carries this check's replay key, and the suite did not mark it owned.
  Traces: elasticache.elasticache-redis-key-expiry-removes-value-and-reports-ttl.

Cause 3 of 6: CreateUser refuses a brand-new UserId whose UserName is `default`, so the RBAC user group pattern AWS requires cannot be built.
  AWS: Smithy models CreateUser with required UserId (pattern ^[a-zA-Z][a-zA-Z0-9\-]*$) and required UserName, error DuplicateUserNameFault (awsQueryError code `DuplicateUserName`, HTTP 400, doc "A user with this username already exists") and UserAlreadyExistsFault (`UserAlreadyExists`, for a repeated UserId). Recorded behavior: AWS ACCEPTS CreateUser with a fresh UserId and UserName `default` — every user group must contain a user whose user name is `default`, so a customer creates its own user named `default` (here with AccessString `off -@all`) and puts it in the group.
  Emulator: the call is refused 400 `DuplicateUserName` with `User default already has the username default.` — the emulator treats the system default user as the sole owner of the user name and rejects any new user that reuses it. Fixed text of that refusal is `User ` + ` already has the username ` + `.`; its variable parts are the existing user's id and the requested user name.
  Why: a client following AWS's documented RBAC setup cannot create the `default`-named member its user group needs, so the whole user-group/serverless-cache authentication flow is unbuildable; the refusal also misuses DuplicateUserName for a case AWS accepts.
  Fix: CreateUser's validation and user state (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_user.rs and the user store): uniqueness belongs on UserId (UserAlreadyExists), not on the user name across the account.
  Traces: elasticache.elasticache-iam-auth-token-opens-a-session-on-a-serverless-cache.

Cause 4 of 6: CreateReplicationGroup refuses a cluster-mode-enabled group that has zero replicas per shard, which is the mandatory shape for cluster mode in AWS.
  AWS: Smithy models CreateReplicationGroup with NumNodeGroups, ReplicasPerNodeGroup and AutomaticFailoverEnabled, error InvalidParameterCombinationException (awsQueryError `InvalidParameterCombination`, HTTP 400). Recorded behavior: AWS ACCEPTS NumNodeGroups>1 with ReplicasPerNodeGroup=0 and AutomaticFailoverEnabled=true (automatic failover is mandatory for cluster mode enabled, and AWS refuses it being disabled there) and settles the group 'available' with the requested shard count.
  Emulator: the create is refused 400 `InvalidParameterCombination` with `AutomaticFailoverEnabled requires at least one replica in every node group (shard) to promote.` — the emulator couples AutomaticFailoverEnabled to a replica count, so a cluster-mode group with no replicas cannot be created and the whole reshard state machine behind it is unreachable.
  Why: a client provisioning a cluster-mode-enabled replication group the AWS way (shards without replicas) is refused outright; it has no accepted combination left, since disabling automatic failover is also refused for cluster mode.
  Fix: CreateReplicationGroup's parameter validation (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_replication_group.rs): the replica-count requirement must not apply to a cluster-mode-enabled create. Related open issue 20261108T093128Z-ec2-elasticache-elasticache-in-vpc-control-plane-diverges-from-aws.md records the WORDING of the single-node automatic-failover refusal; it does not state that a zero-replica cluster-mode create must be accepted.
  Traces: elasticache.shard-configuration-resize-transitions.

Cause 5 of 6: DeleteCacheCluster deletes the primary member of a replication group with automatic failover instead of refusing it.
  AWS: Smithy models DeleteCacheCluster with error InvalidCacheClusterStateFault (awsQueryError `InvalidCacheClusterState`, HTTP 400, doc "The requested cluster is not in the available state"). AWS refuses deleting the primary member of a replication group whose automatic failover is enabled and answers 400 InvalidCacheClusterState with `Cache cluster <cluster-id> is serving as primary for replication group <replication-group-id> and cannot be deleted. To delete the entire replication group, use DeleteReplicationGroup.` — fixed text is everything but the two ids, which come from the request and the group's state; the group's membership and primary must be unchanged afterwards, while the same call on a non-primary member succeeds.
  Emulator: the call SUCCEEDS with HTTP 200 and reports the primary CacheCluster with CacheClusterStatus 'deleting'; no refusal is raised and the group loses its primary.
  Why: a client (or a careless operator) can silently destroy the primary of a highly-available group; code that relies on InvalidCacheClusterState to protect the primary gets no error, and the group is left without the member it was promised.
  Fix: DeleteCacheCluster's state/role validation (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_cache_cluster.rs) must consult the replication-group membership and automatic-failover setting before accepting.
  Traces: elasticache.delete-primary-cluster-automatic-failover-rejected.

Cause 6 of 6: ModifyCacheSubnetGroup refuses a subnet list spanning two VPCs with the wrong error shape.
  AWS: Smithy models ModifyCacheSubnetGroup with both InvalidSubnet (awsQueryError code `InvalidSubnet`, HTTP 400, doc "An invalid subnet identifier was specified") and SubnetNotAllowedFault (`SubnetNotAllowedFault`, 400, doc: at least one subnet ID does not match the other subnet IDs — the regional-AZ vs Outpost mismatch case). AWS answers a cross-VPC subnet list with `InvalidSubnet`.
  Emulator: the refusal carries code `SubnetNotAllowedFault` with `At least one subnet ID does not match the other subnet IDs: <subnet-a> belongs to <vpc-a> and <subnet-b> belongs to <vpc-b>.` — right rejection, wrong modeled error shape (the ids/vpcs in the message are per-run values).
  Why: a client that branches on Error.Code — retrying or re-selecting subnets on InvalidSubnet — takes the wrong branch and surfaces an Outpost-mismatch fault for a plain cross-VPC mistake.
  Fix: ModifyCacheSubnetGroup's subnet validation and its error mapping (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_cache_subnet_group.rs). Related open issue 20261103T220425Z-elasticache-modifycachesubnetgroup-stub.md records the operation as an unimplemented stub; it says nothing about which error shape a cross-VPC list must get.
  Traces: elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected.

## Evidence
Cause 1 (12 checks): Expected — the Endpoint/Address a cache cluster, replication group or serverless cache advertises is an address a client of this emulator can open a RESP/TLS connection to. Actual — `simparity.published.PublishedUrlRejected: endpoint_host was given master.<rg-id>.000001.us-west-2.cache.amazonaws.com:6379 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]`. Same verdict for node endpoints (`<cluster-id>-0001.000001.us-west-2.cache.amazonaws.com:6379`) and serverless endpoints (`<name>-000001.serverless.us-west-2.cache.amazonaws.com:6379`).

Cause 2 (elasticache.elasticache-redis-key-expiry-removes-value-and-reports-ttl): Expected — Invoke of the in-VPC probe returns no FunctionError. Actual — `StatusCode=200 FunctionError='Unhandled': {"errorMessage":"Unable to import module 'index': No module named '_ssl'","errorType":"Runtime.ImportModuleError"}`, reported as `the in-VPC executor is unusable, so this run measured nothing`.

Cause 3 (elasticache.elasticache-iam-auth-token-opens-a-session-on-a-serverless-cache): Expected — CreateUser(UserId=<fresh id>, UserName='default', Engine='valkey', AccessString='off -@all', Passwords=[...]) succeeds. Actual — `botocore.errorfactory.DuplicateUserNameFault: An error occurred (DuplicateUserName) when calling the CreateUser operation: User default already has the username default.`

Cause 4 (elasticache.shard-configuration-resize-transitions): Expected — CreateReplicationGroup(Engine=redis, CacheNodeType=cache.m5.large, NumNodeGroups=2, ReplicasPerNodeGroup=0, AutomaticFailoverEnabled=True) is accepted and settles 'available' with 2 node groups. Actual — `botocore.errorfactory.InvalidParameterCombinationException: An error occurred (InvalidParameterCombination) when calling the CreateReplicationGroup operation: AutomaticFailoverEnabled requires at least one replica in every node group (shard) to promote.`

Cause 5 (elasticache.delete-primary-cluster-automatic-failover-rejected): Expected — 400 InvalidCacheClusterState with `Cache cluster <primary-id> is serving as primary for replication group <rg-id> and cannot be deleted. To delete the entire replication group, use DeleteReplicationGroup.`, group untouched. Actual — `expected a rejection, but the call SUCCEEDED and returned {'CacheCluster': {..., 'CacheClusterStatus': 'deleting', 'ReplicationGroupId': '<rg-id>'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.

Cause 6 (elasticache.subnet-group-replace-semantics-and-cross-vpc-rejected): Expected — Error.Code `InvalidSubnet`. Actual — `ModifyCacheSubnetGroup mixing VPC <vpc-a> and VPC <vpc-b>: expected InvalidSubnet, got SubnetNotAllowedFault ('At least one subnet ID does not match the other subnet IDs: <subnet-b> belongs to <vpc-b> and <subnet-a> belongs to <vpc-a>.')`.

## Steps to Reproduce
Cause 1: CreateCacheSubnetGroup over two private subnets; CreateCacheCluster(Engine=redis, Port=6379) and/or CreateReplicationGroup and/or CreateServerlessCache; wait for 'available'; read the Endpoint/Address from DescribeCacheClusters(ShowCacheNodeInfo=true) / DescribeReplicationGroups (NodeGroups[].PrimaryEndpoint) / DescribeServerlessCaches (Endpoint). Observe the address is an *.cache.amazonaws.com name the emulator does not serve; a connect to it never reaches the emulator.
Cause 2: CreateFunction (python3.12) with a VpcConfig over those subnets and a handler that does `import ssl`; Invoke it. Observe FunctionError='Unhandled' with Runtime.ImportModuleError for `_ssl`.
Cause 3: CreateUser(UserId='<fresh>', UserName='default', Engine='valkey', AccessString='off -@all', Passwords=['<pw>']). Expect success; the emulator answers 400 DuplicateUserName.
Cause 4: CreateReplicationGroup(ReplicationGroupId='<id>', Engine='redis', CacheNodeType='cache.m5.large', NumNodeGroups=2, ReplicasPerNodeGroup=0, AutomaticFailoverEnabled=True, CacheSubnetGroupName='<sng>'). Expect acceptance; the emulator answers 400 InvalidParameterCombination. (Then ModifyReplicationGroupShardConfiguration(NodeGroupCount=3, ApplyImmediately=True) must report Status 'modifying' and converge to 3 shards.)
Cause 5: CreateReplicationGroup(NumCacheClusters=3, AutomaticFailoverEnabled=True); wait 'available'; find the member whose CurrentRole is 'primary'; DeleteCacheCluster(CacheClusterId=<that primary>). Expect 400 InvalidCacheClusterState and an unchanged group; the emulator returns 200 with the primary 'deleting'. Positive control: the same call on a non-primary member succeeds.
Cause 6: CreateCacheSubnetGroup in VPC A, then ModifyCacheSubnetGroup(CacheSubnetGroupName=<that group>, SubnetIds=[<subnet in VPC A>, <subnet in VPC B>]). Expect Error.Code InvalidSubnet; the emulator answers SubnetNotAllowedFault.

## Resolution

Causes 1, 3, 4, 5 and 6 are fixed: every cache (cache cluster, replication-group member,
serverless cache) is now served on a core-owned socket of the emulator's own and publishes
that address, with a RESP server over a real keyspace behind it; `CreateUser` enforces
uniqueness on `UserId` and the user-name rules on the GROUP; a cluster-mode-enabled
replication group takes shards without replicas; `DeleteCacheCluster` refuses a member
serving as a primary; and a cross-VPC subnet list is refused `InvalidSubnet`.

Cause 2 is NOT fixed and is not this service's: the in-VPC RESP probe those checks deploy
runs inside the emulator's Lambda, whose interpreter registers an allow-list of native
modules that deliberately excludes `_socket` and `_ssl`
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python.rs`,
`ALLOWED_NATIVE_MODULES` and the containment note above it), so guest code can open no
connection at all. Giving that runtime a socket and TLS extension is a change to another
service's crate and is tracked by the two open issues this report names
(20261108T152921Z-elasticache-in-vpc-data-plane-unverifiable-the-emulator.md and
20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md). The ten probe-based
data-plane checks now get past the published-endpoint cause and stop at that runtime.
