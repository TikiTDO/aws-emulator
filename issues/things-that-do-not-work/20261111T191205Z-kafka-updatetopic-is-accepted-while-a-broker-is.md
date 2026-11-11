Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.topic-update-controller-moved-error
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.topic-update-controller-moved-error|run#19|AssertionError|assert rejections, "broker reboot completed without exposing any UpdateTopic rejection in this run"
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# kafka UpdateTopic is accepted while a broker is rebooting — no replication-factor-vs-available-brokers refusal, so the broker-shortfall BadRequestException never appears

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTopic

## Description
One net-new root cause for the kafka service.

Cause 1 of 1: UpdateTopic never consults how many of the cluster's brokers are actually available, so a partition-count increase on a topic whose replication factor equals the cluster's broker count is accepted while one broker is rebooting. Failing check: kafka.topic-update-controller-moved-error.

AWS: UpdateTopic (Smithy: com.amazonaws.kafka#UpdateTopic, PUT /v1/clusters/{ClusterArn}/topics/{TopicName}, input PartitionCount: __integer; modeled errors include BadRequestException with httpError 400) reaches the cluster's Apache Kafka control path. Adding partitions requires the topic's replication factor (DescribeTopic$ReplicationFactor) to be placeable on brokers that are up: while a RebootBroker operation is in flight on a provisioned cluster, one broker node is down, so a cluster of N brokers offers only N-1. AWS therefore refuses the update with HTTP 400 BadRequestException and the message 'Replication factor: <topic replication factor> larger than available brokers: <brokers currently available>.' — fixed text 'Replication factor: ' ... ' larger than available brokers: ' ... '.', with both numbers taken from state: the topic's own replication factor and the count of brokers up at that instant (for an RF-3 topic on a 3-broker cluster with one broker rebooting: 'Replication factor: 3 larger than available brokers: 2.'). The only other refusal AWS gives in that window is HTTP 400 BadRequestException "You can't update a topic in UPDATING state." (fixed text, no variable part) when an update accepted a moment earlier still has the topic UPDATING. Once the cluster is ACTIVE again the same UpdateTopic succeeds. The emulator itself already words this exact refusal for CreateTopic, so the wording is not in doubt — recorded behavior plus the service's own existing message.

Emulator: every UpdateTopic issued across three sequential RebootBroker calls on a 3-broker cluster returned 200 for an RF-3 topic, growing PartitionCount each time; not one call was refused. The replay failed on `AssertionError: broker reboot completed without exposing any UpdateTopic rejection in this run`, i.e. the rejection list was empty — so neither the broker-shortfall BadRequestException nor the UPDATING-state BadRequestException was ever produced. UpdateTopic validates only the ARN shape, the topic name, 'PartitionCount or Configs present', the topic's own state gate and partition-count monotonicity; it never looks at the cluster's state or its available broker count. Note: this is NOT the stub reported by the open issue 20261029T175822Z-kafka-updatetopic-stub.md — the handler is implemented and answers 200, it is the broker-availability rule that is missing. It is also distinct from the message-wording causes in 20261104T171616Z-kafka-msk-state-version-conflict-duplicate-topic-and-invalid.md, which do not cover this refusal.

Why: a client that models a rolling broker restart relies on the control plane refusing topic reconfiguration while the cluster is short a broker. Against the emulator the partition addition silently succeeds, so an operator's guard rails (retry/back-off until the cluster is ACTIVE, abort a migration that would land an unplaceable replica set) are never exercised, and the emulator reports a topic layout Apache Kafka could not have produced with the brokers then up. Error-handling paths for the broker-shortfall BadRequestException are dead code against the emulator.

Fix: the UpdateTopic handler's validation, plus the cluster/broker state it reads. The handler must resolve the settled cluster and bound the topic's replication factor by the brokers AVAILABLE at that instant (the cluster's broker count less the broker(s) a REBOOTING_BROKER operation has down), refusing with BadRequestException as above; the cluster state must expose that available-broker count while a reboot operation is in flight, which today it does not (RebootBroker only flips the cluster state and files an operation). CreateTopic's equivalent bound reads the configured broker count only, so the same availability notion is needed there.

Traces: kafka.topic-update-controller-moved-error — creates a 3-broker ACTIVE cluster, creates an RF-3 topic, rolls RebootBroker one broker at a time and calls UpdateTopic with a growing PartitionCount through the window. Relevant files: services/service-com-amazonaws-kafka-kafka/src/ops/update_topic.rs (handle body, lines 191-366 — no cluster-state or broker-availability check), services/service-com-amazonaws-kafka-kafka/src/ops/reboot_broker.rs (lines 269-284, start_broker_reboot), services/service-com-amazonaws-kafka-kafka/src/ops/create_topic.rs (lines 287-301, the existing 'Replication factor: ... larger than available brokers: ...' wording bounded by number_of_broker_nodes).

## Evidence
Expected (AWS): while one of three brokers is rebooting, UpdateTopic(ClusterArn, TopicName, PartitionCount=N+1) on an RF-3 topic -> HTTP 400 BadRequestException, message 'Replication factor: 3 larger than available brokers: 2.' (fixed text 'Replication factor: '/' larger than available brokers: '/'.'; the 3 is the topic's ReplicationFactor and the 2 is the count of brokers up, both from state), or HTTP 400 BadRequestException "You can't update a topic in UPDATING state." when the previous accepted update has not settled. After the cluster returns to ACTIVE the same call succeeds and DescribeTopic reports the incremented PartitionCount.

Actual (emulator): every UpdateTopic call in that window returned HTTP 200 with status UPDATING/ACTIVE and no error at all. Replay detail: File "check_kafka_topic_update_controller_moved_error.py", line 90, in run / assert rejections, "broker reboot completed without exposing any UpdateTopic rejection in this run" / AssertionError: broker reboot completed without exposing any UpdateTopic rejection in this run.

## Steps to Reproduce
1. Boot the emulator and point a kafka client at it.
2. Create a provisioned 3-broker cluster (CreateCluster, kafka.t3.small, 3 subnets) and wait for State=ACTIVE.
3. CreateTopic with PartitionCount=1, ReplicationFactor=3; wait for DescribeTopic Status=ACTIVE.
4. ListNodes to read the BrokerIds, then RebootBroker with BrokerIds=[<one id>].
5. While DescribeCluster reports the reboot in flight, call UpdateTopic(ClusterArn, TopicName, PartitionCount=2).
6. Observe HTTP 200 instead of HTTP 400 BadRequestException 'Replication factor: 3 larger than available brokers: 2.' (or "You can't update a topic in UPDATING state.").
7. Replay kafka.topic-update-controller-moved-error to confirm.
