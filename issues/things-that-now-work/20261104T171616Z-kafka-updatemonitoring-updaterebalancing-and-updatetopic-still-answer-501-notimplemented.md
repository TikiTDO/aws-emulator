Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-mutating-updates-rejected-while-creating kafka.cluster-storage-rebalancing-replication-updates-blocked-while-creating kafka.concurrent-topic-partition-increase-race
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-mutating-updates-rejected-while-creating|run#8|AssertionError|assert not bad, f"expected BadRequestException from CREATING state, got {bad}"
Issue-Key: kafka.cluster-storage-rebalancing-replication-updates-blocked-while-creating|run#20|AssertionError|assert rebalancing_code == "BadRequestException", (
Issue-Key: kafka.concurrent-topic-partition-increase-race|run#15|AssertionError|assert len(successes) == 1, f"expected exactly one racer to succeed, got {outcomes}"
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# UpdateMonitoring, UpdateRebalancing and UpdateTopic still answer 501 NotImplemented on paths where MSK answers a modeled client error or accepts the call (3 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMonitoring

## Description
THREE distinct root causes: three cluster/topic mutation operations are scaffold stubs that answer HTTP 501 NotImplemented, which is not a modeled response for any of them, so the state gates and concurrency behaviour that sit behind them cannot be observed. Each operation is a separate handler and must be closed separately; all three must be closed before this issue is submitted. Related open issues: 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md (four other operations), 20261029T175822Z-kafka-updatetopic-stub.md and the UpdateMonitoring stub issue filed from the checkout — none of them carries the three check ids below, which fail on these operations inside multi-operation state-gate and race checks.

Cause 1 of 3: UpdateMonitoring answers NotImplemented where MSK refuses a CREATING cluster with BadRequestException.
AWS: UpdateMonitoring is a cluster-mutating operation whose modeled errors are client/server faults (BadRequestException 400 among them); on a cluster that is still CREATING it is refused with BadRequestException and MSK's fixed state sentence, and a stale CurrentVersion is likewise a modeled 400 — never a 501. On an ACTIVE cluster it is accepted and returns a ClusterOperationArn whose operation is observable as PENDING/UPDATE_IN_PROGRESS/UPDATE_COMPLETE.
Emulator: 'NotImplemented' for both the CREATING-state call and the stale-version call ({'UpdateMonitoring': 'NotImplemented', 'UpdateMonitoring(staleVersion)': 'NotImplemented'}), so neither the state gate nor the version gate exists.
Why: 501 is not in the operation's error set, so an SDK surfaces an unmodeled failure; a client cannot distinguish 'wait for ACTIVE' from 'this service cannot do it', and the state-gate contract for monitoring updates is unverifiable.
Fix: implement the UpdateMonitoring handler (services/service-com-amazonaws-kafka-kafka/src/ops/update_monitoring.rs), including the cluster-state and CurrentVersion preconditions and the ClusterOperation it must mint.
Traces: kafka.cluster-mutating-updates-rejected-while-creating.

Cause 2 of 3: UpdateRebalancing answers NotImplemented where MSK refuses a CREATING cluster with BadRequestException.
AWS: UpdateRebalancing on a cluster that is not ACTIVE is refused with BadRequestException and MSK's fixed state sentence ('This operation is only valid for resources that are in one of the following states :[ACTIVE]'), and the cluster's CurrentVersion is unchanged by the refusal.
Emulator: 'NotImplemented'.
Why: as above — the refusal class and the no-side-effect guarantee are both unobservable, and a client sees an unmodeled 501.
Fix: implement the UpdateRebalancing handler with the cluster-state precondition shared with the other mutation paths.
Traces: kafka.cluster-storage-rebalancing-replication-updates-blocked-while-creating.

Cause 3 of 3: UpdateTopic answers NotImplemented, so a partition-count increase can never be applied or raced.
AWS: two concurrent UpdateTopic calls raising one topic from 1 to 3 partitions admit exactly one winner; the loser is refused with BadRequestException, and afterwards the topic has exactly 3 partitions (DescribeTopicPartitions).
Emulator: both racers get ('error', 'NotImplemented') — no partition change is possible at all.
Why: the topic-management surface is advertised but cannot mutate a topic; a client's scale-out path fails with an unmodeled 501 and the concurrency contract is untested.
Fix: implement the UpdateTopic handler (services/service-com-amazonaws-kafka-kafka/src/ops/update_topic.rs), including the single-winner serialization of concurrent partition increases.
Traces: kafka.concurrent-topic-partition-increase-race.

## Evidence
All three checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED (the stub issues filed from the checkout carry other check ids).

Cause 1 — kafka.cluster-mutating-updates-rejected-while-creating: AssertionError 'expected BadRequestException from CREATING state, got {\'UpdateMonitoring\': \'NotImplemented\', \'UpdateMonitoring(staleVersion)\': \'NotImplemented\'}'. The same check's sibling operations did answer BadRequestException, so UpdateMonitoring is the only gap it reports.

Cause 2 — kafka.cluster-storage-rebalancing-replication-updates-blocked-while-creating: AssertionError 'expected BadRequestException for UpdateRebalancing on a CREATING cluster, got NotImplemented'.

Cause 3 — kafka.concurrent-topic-partition-increase-race: two threads calling UpdateTopic(ClusterArn, TopicName, PartitionCount=3) behind a barrier.
  Expected: exactly one 'ok', one BadRequestException, and 3 partitions afterwards.
  Actual: AssertionError 'expected exactly one racer to succeed, got [(\'error\', \'NotImplemented\'), (\'error\', \'NotImplemented\')]'.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: CreateClusterV2 a PROVISIONED cluster; while DescribeClusterV2 reports CREATING, call UpdateMonitoring with the cluster's CurrentVersion and assert 400 BadRequestException (not 501 NotImplemented); repeat with a deliberately stale CurrentVersion and assert a modeled 400. Wait for ACTIVE and assert UpdateMonitoring is accepted and returns a ClusterOperationArn whose DescribeClusterOperation state is PENDING/UPDATE_IN_PROGRESS/UPDATE_COMPLETE.
3. Cause 2: while a cluster is CREATING, call UpdateRebalancing with its CurrentVersion and assert 400 BadRequestException with MSK's state sentence, and that CurrentVersion is unchanged.
4. Cause 3: create a topic with 1 partition on an ACTIVE cluster, wait for ACTIVE, then from two threads call UpdateTopic(PartitionCount=3). Assert exactly one call succeeds, the other is BadRequestException, and DescribeTopicPartitions then reports 3 partitions.
5. Replay kafka.cluster-mutating-updates-rejected-while-creating, kafka.cluster-storage-rebalancing-replication-updates-blocked-while-creating and kafka.concurrent-topic-partition-increase-race.
