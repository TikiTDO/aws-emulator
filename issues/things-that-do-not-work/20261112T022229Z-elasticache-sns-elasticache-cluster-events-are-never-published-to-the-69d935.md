Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber
Issue-Key: elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber|_notifications#1|Timeout|batch = sqs.receive_message(
Issue-Commit: 8a5c50c8a2e4f68c625b1ffaca3d8c020fa0b343

# ElastiCache cluster events are never published to the cluster's configured SNS topic (1 cause, 1 check)

> _Note: this issue was identified against the code base at commit `8a5c50c8a2e4f68c625b1ffaca3d8c020fa0b343`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
One net-new root cause on the IMPLEMENTED ElastiCache cache-cluster surface, exercised by the single failing check elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber.

Note on existing issues: open issue 20261109T054811Z-elasticache-sns-elasticache-cluster-events-are-never-published-to-the.md describes this same cause (as its Cause 1 of 2) and carries this same check id, but the replay report did NOT mark this check ALREADY FILED or OWNED, so it is re-filed here as instructed; whoever fixes either should close both together. That issue's second cause (ModifyCacheCluster with NotificationTopicStatus='inactive' clearing the stored topic ARN and a hardcoded 'active' TopicStatus) appears to have been addressed in this checkout — state.rs now documents NOTIFICATION_STATUS_ACTIVE / NOTIFICATION_STATUS_INACTIVE and says the status is held PER CLUSTER on CacheClusterRecord::notification_topic_status — so it is NOT re-filed; only the publish path below is.

Cause 1 of 1: a cache cluster that carries a NotificationTopicArn publishes NOTHING to that SNS topic; cluster events reach only ElastiCache's own event log.
AWS: Smithy (service-com-amazonaws-elasticache-amazonelasticachev9): CreateCacheCluster$NotificationTopicArn — "The Amazon Resource Name (ARN) of the Amazon Simple Notification Service (SNS) topic to which notifications are sent. The Amazon SNS topic owner must be the same as the cluster owner"; ModifyCacheCluster$NotificationTopicStatus — "The status of the Amazon SNS notification topic ... Notifications are sent only if the status is active. Valid values: active | inactive"; DescribeCacheClusters projects CacheCluster$NotificationConfiguration as {TopicArn, TopicStatus}. The publish itself is recorded behavior only (AmazonElastiCache ECEvents.SNS): while NotificationConfiguration.TopicStatus is 'active', every cluster event ElastiCache records for that cluster is ALSO published to the named topic, so an SQS subscriber of that topic receives an SNS envelope with Type='Notification' and TopicArn = the configured topic, whose Message carries the documented `ElastiCache:<Event>` event name together with the cluster id (a node reboot produces the ElastiCache:CacheNodeReboot* events). Fixed parts of the requirement: the envelope shape, Type='Notification', the `ElastiCache:` prefix on the event name. Variable parts, taken from the request and from state: the topic ARN, the cluster id, the event name and the timestamps — never the particular ids of one recording.
Emulator: the ARN is stored and nothing else happens. create_cache_cluster.rs copies input.notification_topic_arn onto the cluster record and calls store.record_event(..); reboot_cache_cluster.rs drives the node/cluster transition and states that "the cluster event the reboot's completion creates is recorded — and published to the cluster's topic — when the clock materialises that completion, which is where DescribeEvents reads it from", but nothing on the ElastiCache event path ever issues an SNS Publish: the whole of state.rs mentions SNS once, in a doc comment about NotificationTopicStatus's values. DescribeEvents does record the reboot, so the producer side is observable while the delivery side stays permanently silent and the subscriber queue's ApproximateNumberOfMessages stays 0.
Why: a client wiring cluster events to SNS (the documented ElastiCache→SNS→SQS/Lambda alerting pattern) is told NotificationConfiguration.TopicStatus='active' for its own topic and then never receives a message — no error, just silence. It never learns that a node rebooted, that provisioning finished, or that a failover happened, and an absence-based test of "notifications inactive" passes for the wrong reason because there is no delivery path at all.
Fix: the elasticache cluster-event path — where a cluster event becomes real (crate::state's event recording and CacheClusterRecord::settle, plus the create/modify/reboot/delete handlers that drive it) must publish the event to the cluster's configured topic whenever that topic's status is active, not merely append to the local event log.
Traces: elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber — arm A reboots the node (RebootCacheCluster with the node ids DescribeCacheClusters(ShowCacheNodeInfo=true) reports), asserts the reboot IS in DescribeEvents, and then waits for a matching SNS notification on its own queue; the wait cannot terminate and the harness kills the check at its 60s ceiling.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: 451/602 passed (74.9%). Service elasticache__sns 1/2 (50%), 1 failed.

Failing check (NOT marked ALREADY FILED and NOT marked OWNED in this report):
  [mismatch] elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber
  Error: Check timed out after 60.0s

Expected (AWS): after CreateCacheCluster(..., NotificationTopicArn=<topic>) settles 'available' with NotificationConfiguration={TopicArn:<topic>, TopicStatus:'active'} and a RebootCacheCluster completes, an SQS subscriber of <topic> receives an SNS envelope with Type='Notification', TopicArn=<topic> and a Message containing `ElastiCache:<Event>` plus the cluster id; the queue's ApproximateNumberOfMessages becomes >= 1.
Actual (emulator): DescribeEvents records the reboot event, the queue stays at depth 0 forever and no envelope is ever delivered, so the check's _await_notification loop (600s budget) burns the whole 60s per-check ceiling.

Code evidence in this checkout:
- services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_cache_cluster.rs: notification_topic_arn is stored on the cluster record; the handler records an event; no SNS publish.
- services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/reboot_cache_cluster.rs lines 231-246: the completion event is recorded for DescribeEvents only.
- services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs: the only occurrence of 'SNS' in the entire 4547-line file is the doc comment at lines 872-880 for NOTIFICATION_STATUS_ACTIVE / NOTIFICATION_STATUS_INACTIVE — there is no publish path anywhere on the event path.

Harness note for re-replay: the check's own absence window has a 240s floor (_ABSENCE_FLOOR_S) above the suite's 60s per-check ceiling, so it cannot report PASS inside that ceiling even once the publish lands. Verify on the observables instead: (a) after a reboot of a cluster with an active notification topic, an SQS subscriber of that topic receives an SNS Notification envelope naming that topic whose Message carries `ElastiCache:<Event>` and the cluster id; (b) with NotificationTopicStatus='inactive' no further message arrives while DescribeEvents keeps recording; (c) after setting it back to 'active', delivery to the same topic resumes.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. sns CreateTopic; sqs CreateQueue; sqs SetQueueAttributes with a Policy allowing sns.amazonaws.com sqs:SendMessage on that queue for that topic ARN; sns Subscribe(Protocol='sqs', Endpoint=<queue arn>). Prove the pipe with a plain sns Publish that arrives on the queue.
3. ec2 CreateVpc + CreateSubnet (one per available AZ) + CreateSecurityGroup; elasticache CreateCacheSubnetGroup over those subnets.
4. elasticache CreateCacheCluster(CacheClusterId=<id>, Engine='redis', CacheNodeType='cache.t3.micro', NumCacheNodes=1, Port=6379, CacheSubnetGroupName=<group>, SecurityGroupIds=[<sg>], NotificationTopicArn=<topic>); poll DescribeCacheClusters until CacheClusterStatus='available' and NotificationConfiguration={TopicArn:<topic>, TopicStatus:'active'}.
5. elasticache RebootCacheCluster(CacheClusterId=<id>, CacheNodeIdsToReboot=[the node ids from DescribeCacheClusters(ShowCacheNodeInfo=true)]).
6. elasticache DescribeEvents(SourceIdentifier=<id>, SourceType='cache-cluster') — the reboot event IS recorded.
7. sqs ReceiveMessage and sqs GetQueueAttributes(AttributeNames=['ApproximateNumberOfMessages']) — expect at least one SNS Notification envelope whose TopicArn is <topic> and whose Message carries `ElastiCache:<Event>` and <id>; observe nothing is ever delivered and the depth stays 0.
8. Re-replay elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber (see the harness note about the 240s absence floor vs the 60s ceiling).
