Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.migration-complete-without-active-migration-rejected elasticache.online-migration-lifecycle elasticache.replication-group-migration-lifecycle elasticache.modify-shard-configuration-disabled-cluster-mode-rejected elasticache.global-replication-group-secondary-lifecycle elasticache.create-serverless-cache-conflicting-name-rejected elasticache.create-serverless-cache-usage-limit-out-of-range-rejected elasticache.e2e-serverless-snapshot-restore-flow elasticache.modify-cache-cluster-pending-vs-immediate-application elasticache.replication-group-auth-token-rotation-grace-window elasticache.deploy-cache-cluster-cfn-behaves
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.migration-complete-without-active-migration-rejected|run#13|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticache.online-migration-lifecycle|run#12|AssertionError|assert code == "InvalidParameterValue", code
Issue-Key: elasticache.replication-group-migration-lifecycle|run#13|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Key: elasticache.modify-shard-configuration-disabled-cluster-mode-rejected|run#15|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Key: elasticache|CreateReplicationGroup|NotImplemented
Issue-Key: elasticache.create-serverless-cache-conflicting-name-rejected|_wait_serverless_cache_available#5|Timeout|caches = ec.describe_serverless_caches(ServerlessCacheName=name)["ServerlessCaches"]
Issue-Key: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: elasticache.e2e-serverless-snapshot-restore-flow|_wait_serverless_cache_available#9|Timeout|caches = ec.describe_serverless_caches(ServerlessCacheName=name)["ServerlessCaches"]
Issue-Key: elasticache.modify-cache-cluster-pending-vs-immediate-application|_wait_for_apply_status#12|Timeout|time.sleep(2)
Issue-Key: elasticache.replication-group-auth-token-rotation-grace-window|_poll_until_pending_auth_status#14|Timeout|time.sleep(interval_s)
Issue-Key: elasticache|DescribeCacheClusters|CacheClusterNotFound
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# ElastiCache migration, reshard, global-secondary, serverless and pending-change surfaces never answer or never converge (7 causes)

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteMigration

## Description
Seven independent root causes where a modeled ElastiCache surface either answers the scaffold's NotImplemented or never converges to the state a client must poll for, so the whole flow behind it is unreachable. Each must be closed separately. Related open issue: 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md lists some of these check ids under its Causes 5-7 but the replay report binds none of them to it (no owner reported), so they are re-grounded here with the signatures observed on THIS checkout; closing that issue's causes should close Causes 1, 2 and 4 here.

Cause 1 of 7: CompleteMigration is unimplemented and answers NotImplemented instead of the modeled migration refusal.
AWS: Smithy: CompleteMigration models InvalidParameterValueException (awsQueryError code 'InvalidParameterValue', 400), ReplicationGroupNotUnderMigrationFault, InvalidReplicationGroupStateFault and ReplicationGroupNotFoundFault (404). Recorded behavior on an available group with no migration in flight, with and without Force=True: HTTP 400 InvalidParameterValue with Message 'The replication group must be in migrating state to complete the migration. Please retry later.' (whole string fixed), group left available.
Emulator: every call answers code 'NotImplemented' with 'operation ... is scaffolded but not implemented in this emulator'.
Why: the online-migration state machine cannot be driven or even probed; a client cannot distinguish 'no migration in flight' from 'this service does not implement migration', and the Start->Complete ordering contract is unenforced.
Fix: implement the CompleteMigration handler over the replication-group migration state in the elasticache crate.
Traces: elasticache.migration-complete-without-active-migration-rejected, elasticache.online-migration-lifecycle, elasticache.replication-group-migration-lifecycle.

Cause 2 of 7: ModifyReplicationGroupShardConfiguration is unimplemented and answers NotImplemented.
AWS: Smithy: ModifyReplicationGroupShardConfiguration requires ReplicationGroupId/NodeGroupCount/ApplyImmediately and models InvalidParameterValueException ('InvalidParameterValue', 400), InvalidParameterCombinationException, InvalidReplicationGroupStateFault and ReplicationGroupNotFoundFault (404). Recorded behavior for a cluster-mode-DISABLED group: HTTP 400 InvalidParameterValue; a consistent scale-out on a cluster-mode-enabled group is accepted with 200.
Emulator: code 'NotImplemented' for every call, so neither the refusal nor the accepted reshard exists.
Why: resharding is the only way to change a replication group's shard count; a client cannot scale, and cannot learn that cluster mode is a precondition.
Fix: implement the ModifyReplicationGroupShardConfiguration handler (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_replication_group_shard_configuration.rs), its validation ordering and its node-group transition.
Traces: elasticache.modify-shard-configuration-disabled-cluster-mode-rejected.

Cause 3 of 7: CreateReplicationGroup as the SECONDARY of a Global datastore is a scaffolded path that answers NotImplemented.
AWS: Smithy: CreateReplicationGroup takes GlobalReplicationGroupId and models GlobalReplicationGroupNotFoundFault / InvalidGlobalReplicationGroupStateFault; the operation itself is implemented. Recorded behavior: CreateReplicationGroup(GlobalReplicationGroupId=<grg>, ReplicationGroupId=..., in the secondary Region) returns HTTP 200 and the group joins the global datastore as a secondary.
Emulator: HTTP 400/501 with code 'NotImplemented' and Message 'operation CreateReplicationGroup as the secondary of a Global datastore is scaffolded but not implemented in this emulator' — the operation answers normally for every other shape, only this input path is a stub.
Why: a Global datastore can never have a secondary, so cross-Region replication, failover-over-regions and secondary disassociation are all unreachable.
Fix: the GlobalReplicationGroupId branch of the CreateReplicationGroup handler in the elasticache crate (join the named global group and project the member).
Traces: elasticache.global-replication-group-secondary-lifecycle.

Cause 4 of 7: serverless cache Status is serialized in UPPER case, so every serverless waiter hangs.
AWS: recorded behavior — CreateServerlessCache answers 200 with ServerlessCache.Status 'creating' and the cache then reports 'available'; a serverless snapshot moves 'creating' -> 'available'. (The model's prose for ServerlessCache$Status lists CREATING/AVAILABLE/..., but the recorded wire value is lower case, so the recording governs.)
Emulator: returns Status 'CREATING' and never reports a lower-case lifecycle value, so a client polling for 'available' never converges; three checks here are killed by the 180 s harness ceiling inside that poll (and the still-failing elasticache.serverless-cache-lifecycle asserts the raw 'CREATING' directly, which is the direct evidence for the casing).
Why: every serverless flow — create, conflicting-name refusal, usage-limit refusal, snapshot/restore — is unreachable in practice, and the SDK's documented waiter pattern hangs instead of failing fast.
Fix: the ServerlessCache / ServerlessCacheSnapshot status values and transitions in the elasticache state projection.
Traces: elasticache.create-serverless-cache-conflicting-name-rejected, elasticache.create-serverless-cache-usage-limit-out-of-range-rejected, elasticache.e2e-serverless-snapshot-restore-flow.

Cause 5 of 7: ModifyCacheCluster without ApplyImmediately never produces a 'pending-reboot' parameter-apply state.
AWS: Smithy: ModifyCacheCluster takes CacheParameterGroupName and ApplyImmediately; CacheCluster.CacheParameterGroup is a CacheParameterGroupStatus carrying ParameterApplyStatus and CacheNodeIdsToReboot. Recorded behavior: with ApplyImmediately=False the cluster stays 'available', CacheParameterGroup.CacheParameterGroupName flips to the NEW group with ParameterApplyStatus 'pending-reboot' and CacheNodeIdsToReboot naming the node(s); RebootCacheCluster then moves it to 'in-sync'.
Emulator: the check's 180 s poll for ParameterApplyStatus 'pending-reboot' never sees it (the run ends inside that poll). Hypothesis: the handler either applies the new parameter group immediately or leaves ParameterApplyStatus 'in-sync', so the pending-vs-immediate distinction does not exist.
Why: the deferred-change contract is how a client schedules a disruptive parameter change into its maintenance window; without 'pending-reboot' and CacheNodeIdsToReboot it cannot tell a queued change from an applied one, and RebootCacheCluster has nothing to apply.
Fix: the ModifyCacheCluster handler's pending-modified-values path and the CacheParameterGroupStatus projection in the elasticache crate.
Traces: elasticache.modify-cache-cluster-pending-vs-immediate-application.

Cause 6 of 7: an AuthToken ROTATE never reports PendingModifiedValues.AuthTokenStatus='ROTATING'.
AWS: Smithy: ModifyReplicationGroup takes AuthToken and AuthTokenUpdateStrategy; ReplicationGroupPendingModifiedValues carries AuthTokenStatus. Recorded behavior: after ModifyReplicationGroup(AuthToken=<new>, AuthTokenUpdateStrategy='ROTATE', ApplyImmediately=True) DescribeReplicationGroups reports PendingModifiedValues.AuthTokenStatus 'ROTATING' (the dual-token grace window), and a following SET finalizes the rotation and clears the status.
Emulator: the 300 s poll for AuthTokenStatus 'ROTATING' never observes it (the run ends inside that poll; the group itself does reach 'available', since sibling checks modify it successfully). Hypothesis: ModifyReplicationGroup accepts the ROTATE but projects no PendingModifiedValues.AuthTokenStatus.
Why: the grace window is the only way a client knows both tokens are currently valid; without it a rotation cannot be sequenced safely and the finalizing SET cannot be timed.
Fix: the ModifyReplicationGroup AuthTokenUpdateStrategy handling and the ReplicationGroupPendingModifiedValues projection in the elasticache crate.
Traces: elasticache.replication-group-auth-token-rotation-grace-window.

Cause 7 of 7: a CloudFormation stack containing AWS::ElastiCache::CacheCluster reaches CREATE_COMPLETE without creating the cluster.
AWS: recorded behavior — a stack whose only resource is AWS::ElastiCache::CacheCluster reaches CREATE_COMPLETE only once the cluster exists; DescribeCacheClusters on the physical id then returns it (and it accepts a live ModifyCacheCluster).
Emulator: the stack settles, then DescribeCacheClusters on the named cluster raises 404 CacheClusterNotFound ('CacheCluster not found: <id>') — the stack resource was never materialized in the elasticache store.
Why: every CloudFormation-driven cache deployment reports success and leaves nothing behind, so a client's post-deploy wiring (endpoints, modifications, teardown) fails on a resource CFN said it created.
Fix: the AWS::ElastiCache::CacheCluster resource provider in the cloudformation crate — it must call through to the elasticache CreateCacheCluster path (same shape as the ReplicationGroup/ServerlessCache gap in open issue 20261104T091523Z-elasticache-via-cloudformation-endpoint-outputs-are-returned-with.md, which owns the ReplicationGroup/ServerlessCache checks but not this one).
Traces: elasticache.deploy-cache-cluster-cfn-behaves.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout (68/161 passed). Expected = recorded AWS behavior, actual = emulator:
1. migration-complete-without-active-migration-rejected: AssertionError: unexpected rejection code for CompleteMigration without an active migration: 'NotImplemented'. online-migration-lifecycle: AssertionError: NotImplemented (assert code == 'InvalidParameterValue' on CompleteMigration before any migration started). replication-group-migration-lifecycle: AssertionError: expected InvalidParameterValue, got NotImplemented (CompleteMigration(Force=True)). Expected 400 InvalidParameterValue 'The replication group must be in migrating state to complete the migration. Please retry later.'
2. modify-shard-configuration-disabled-cluster-mode-rejected: ModifyReplicationGroupShardConfiguration(ReplicationGroupId=<cluster-mode-disabled>, NodeGroupCount=2, ApplyImmediately=True) -> AssertionError: expected InvalidParameterValue, got NotImplemented.
3. global-replication-group-secondary-lifecycle: AssertionError: could not create secondary replication group in us-east-1: An error occurred (NotImplemented) when calling the CreateReplicationGroup operation: operation CreateReplicationGroup as the secondary of a Global datastore is scaffolded but not implemented in this emulator.
4. create-serverless-cache-conflicting-name-rejected / create-serverless-cache-usage-limit-out-of-range-rejected / e2e-serverless-snapshot-restore-flow: 'Check timed out after 180.0s' inside _wait_serverless_cache_available / _wait_serverless_status. Direct casing evidence from the same run: elasticache.serverless-cache-lifecycle fails with 'AssertionError: CREATING' (expected one of creating/available) and deploy-serverless-cache-cfn-invalid-engine-version-rejected shows a CreateServerlessCache 200 body carrying 'Status': 'CREATING'.
5. modify-cache-cluster-pending-vs-immediate-application: 'Check timed out after 180.0s'; the check creates two memcached parameter groups, a cluster on group A, waits for available, calls ModifyCacheCluster(CacheParameterGroupName=group B, ApplyImmediately=False) and then polls DescribeCacheClusters for CacheParameterGroup.ParameterApplyStatus == 'pending-reboot' with CacheNodeIdsToReboot populated (timeout_s=180). Expected: 'pending-reboot' + node ids while CacheClusterStatus stays 'available', then 'in-sync' after RebootCacheCluster.
6. replication-group-auth-token-rotation-grace-window: 'Check timed out after 180.0s'; the check reaches ModifyReplicationGroup(AuthToken=<token2>, AuthTokenUpdateStrategy='ROTATE', ApplyImmediately=True) and then polls DescribeReplicationGroups for PendingModifiedValues.AuthTokenStatus == 'ROTATING' (timeout_s=300).
7. deploy-cache-cluster-cfn-behaves: botocore.errorfactory.CacheClusterNotFoundFault: An error occurred (CacheClusterNotFound) when calling the DescribeCacheClusters operation: CacheCluster not found: sim2real-cc-0d201bdf45a1 — after the CFN stack carrying AWS::ElastiCache::CacheCluster settled.
Model evidence: CompleteMigration binds InvalidParameterValueException -> {InvalidParameterValue, 400}, ReplicationGroupNotUnderMigrationFault and ReplicationGroupNotFoundFault -> 404; ModifyReplicationGroupShardConfiguration requires ReplicationGroupId/NodeGroupCount/ApplyImmediately and binds InvalidParameterValueException -> {InvalidParameterValue, 400}; CreateReplicationGroup takes GlobalReplicationGroupId and binds GlobalReplicationGroupNotFoundFault -> 404. 'NotImplemented' is not a shape in this model.

## Steps to Reproduce
1. Boot the emulator from this checkout and run suite simparity-behavioral-aws-v1.
2. Cause 1: on an available replication group, CompleteMigration(ReplicationGroupId=...) and CompleteMigration(..., Force=True) -> expect 400 InvalidParameterValue 'The replication group must be in migrating state to complete the migration. Please retry later.'; observe NotImplemented. Re-replay elasticache.migration-complete-without-active-migration-rejected, elasticache.online-migration-lifecycle, elasticache.replication-group-migration-lifecycle.
3. Cause 2: on a cluster-mode-disabled replication group, ModifyReplicationGroupShardConfiguration(NodeGroupCount=2, ApplyImmediately=True) -> expect 400 InvalidParameterValue; observe NotImplemented. Re-replay elasticache.modify-shard-configuration-disabled-cluster-mode-rejected.
4. Cause 3: CreateGlobalReplicationGroup from a primary, then CreateReplicationGroup(GlobalReplicationGroupId=<grg>, ReplicationGroupId=..., in a second Region) -> expect 200 and a secondary member; observe NotImplemented. Re-replay elasticache.global-replication-group-secondary-lifecycle.
5. Cause 4: CreateServerlessCache(ServerlessCacheName=..., Engine='redis') -> expect Status 'creating' then DescribeServerlessCaches 'available'; observe 'CREATING' forever. Re-replay elasticache.create-serverless-cache-conflicting-name-rejected, elasticache.create-serverless-cache-usage-limit-out-of-range-rejected, elasticache.e2e-serverless-snapshot-restore-flow.
6. Cause 5: create two memcached cache parameter groups, create a cluster on the first, wait available, ModifyCacheCluster(CacheParameterGroupName=<second>, ApplyImmediately=False), then DescribeCacheClusters -> expect CacheParameterGroup.ParameterApplyStatus 'pending-reboot' with CacheNodeIdsToReboot set and CacheClusterStatus still 'available'; then RebootCacheCluster -> expect 'in-sync'. Re-replay elasticache.modify-cache-cluster-pending-vs-immediate-application.
7. Cause 6: create a redis replication group with TransitEncryptionEnabled=True and an AuthToken, wait available, ModifyReplicationGroup(AuthToken=<new>, AuthTokenUpdateStrategy='ROTATE', ApplyImmediately=True), then DescribeReplicationGroups -> expect PendingModifiedValues.AuthTokenStatus 'ROTATING', then a SET rotation clears it. Re-replay elasticache.replication-group-auth-token-rotation-grace-window.
8. Cause 7: create a CFN stack whose only resource is AWS::ElastiCache::CacheCluster, wait for CREATE_COMPLETE, then DescribeCacheClusters on that CacheClusterId -> expect the cluster; observe 404 CacheClusterNotFound. Re-replay elasticache.deploy-cache-cluster-cfn-behaves.
