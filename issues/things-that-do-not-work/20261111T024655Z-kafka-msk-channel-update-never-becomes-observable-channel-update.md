Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-update-mutation-reflected-in-describe
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-update-mutation-reflected-in-describe|_wait_for_channel_config_converged#18|Timeout|info = kafka.describe_channel(ChannelArn=channel_arn, ClusterArn=cluster_arn)
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# MSK channel update never becomes observable: channel-update-mutation-reflected-in-describe reaches no verdict (60 s timeout) because UpdateChannel's mutation never converges in DescribeChannel

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
One kafka behavioral check survives every already-filed / owned filter in this replay and is the sole net-new cause here.

Cause 1 of 1: an MSK data-delivery channel on an Express provisioned cluster never reaches a terminal, mutated state that an independent DescribeChannel can observe, so the whole CreateChannel -> wait ACTIVE -> UpdateChannel -> re-read sequence runs to its deadline and the check reports only 'Error: Check timed out after 60.0s' — no HTTP status, no error code, no traceback frame, nothing asserted.

AWS: Smithy (com.amazonaws.kafka#Kafka): CreateChannel returns a channelArn plus a clusterOperationArn; DescribeChannel (GET /v1/clusters/{ClusterArn}/channels/{ChannelArn}, 200) REQUIRES channelArn, channelName, status, destinationType, creationTime and topicConfigurationList, with status drawn from the ChannelStatus enum CREATING|ACTIVE|UPDATING|DELETING|FAILED|SUSPENDING|SUSPENDED, so a freshly created channel settles CREATING -> ACTIVE. S3DestinationConfiguration.DataFreshnessInSeconds is documented 'Allowed range: 300 to 900. Default: 600.' UpdateChannel (PUT /v1/clusters/{ClusterArn}/channels/{ChannelArn}, idempotent, 200) takes S3DestinationUpdate with REQUIRED DataFreshnessInSeconds (same 300..900 range) and its modeled output is only {channelArn, clusterOperationArn} — it echoes no destination field at all, so AWS's contract is that the new value becomes visible through an independent DescribeChannel/ListChannels once the channel returns to ACTIVE (UPDATING -> ACTIVE). ListChannels must then list that channel exactly once with Status ACTIVE. The modeled refusals for both ops are BadRequestException 400, ForbiddenException 403, NotFoundException 404, UnauthorizedException 401, TooManyRequestsException 429, InternalServerErrorException 500, ServiceUnavailableException 503 — none of which the emulator produced here.

Emulator: the sequence never yields any response the check can assert on. It creates an Express (express.m7g.large, 3 client subnets) provisioned cluster and waits ACTIVE, creates a topic and waits ACTIVE, creates the channel and polls DescribeChannel at a 10 s cadence for Status ACTIVE, then calls UpdateChannel with S3DestinationUpdate.DataFreshnessInSeconds=900 and polls DescribeChannel at 10 s for (Status ACTIVE AND S3DestinationConfiguration.DataFreshnessInSeconds == 900). The replay output is the bare frameless 'Error: Check timed out after 60.0s'. Hypothesis (neither the model nor the replay output states which link breaks): one of (a) the Express cluster's CREATING -> ACTIVE, (b) the topic's CREATING -> ACTIVE, (c) the channel's CREATING -> ACTIVE, (d) the UpdateChannel handler applying S3DestinationUpdate to the stored channel at all, or (e) the channel's UPDATING -> ACTIVE after the update, never reaches its terminal value, so each poll runs to its deadline rather than paying one interval. Note that UpdateChannel is also on file as an unimplemented scaffold stub (20261104T171616Z-kafka-updatechannel-stub.md), and ListChannels as another (20261029T175822Z-kafka-listchannels-stub.md); a 501 from either would be a response the check could report, which is NOT what happens — it gets no verdict, so settling the states and applying the mutation is a requirement beyond those stubs.

Why: a client that updates a channel's S3 delivery buffering and then waits for the change to take effect hangs until its own timeout. Because UpdateChannel's response carries no destination fields, DescribeChannel/ListChannels are the ONLY way a client can confirm the mutation landed; with neither converging, the emulator gives a caller no way to distinguish 'applied' from 'silently dropped', and the ACTIVE/UPDATING channel lifecycle plus ListChannels' Status projection are unverifiable.

Fix: the channel state machine in the kafka service's state layer, plus the CreateChannel / DescribeChannel / UpdateChannel / ListChannels handlers and the cluster-operation settling they depend on: CreateChannel's channel must settle CREATING -> ACTIVE, UpdateChannel must write S3DestinationUpdate.DataFreshnessInSeconds into the stored channel's S3DestinationConfiguration and settle UPDATING -> ACTIVE, and DescribeChannel/ListChannels must project the mutated value, all within one or two probes at an AWS-sized cadence.

Related open issues that do NOT carry this check's replay key (the suite reports it unowned, so none of them covers it; naming them only so a fixer sees the neighbourhood): 20261110T144707Z-kafka-five-msk-lifecycle-families-still-reach-no-verdict.md lists this check id in its prose but the suite binds no key for it, 20261104T171616Z-kafka-updatechannel-stub.md, 20261029T175822Z-kafka-listchannels-stub.md, 20261108T153215Z-kafka-updatechannel-on-a-deleted-channel-answers-400-badrequestexception.md, 20261106T132758Z-kafka-msk-channel-refusals-invert-notfound-badrequest-two-cluster.md.

Traces: kafka.channel-update-mutation-reflected-in-describe.

## Evidence
Failing check (verdict 'mismatch'), kafka service heading in the suite report:
  kafka.channel-update-mutation-reflected-in-describe — 'Error: Check timed out after 60.0s'

That is the complete detail: no HTTP status line, no error code, no exception frame, hence no assertion was ever evaluated and no refusal was served. Suite totals for this replay: 104/228 passed (45.6%); kafka 103/220 with 117 failures. This is the ONLY failing check in the whole replay that is neither marked ALREADY FILED nor OWNED by an open issue.

Expected (AWS), from the check's own asserts and the Smithy model:
  1. DescribeChannel on the new channel reaches Status 'ACTIVE' with S3DestinationConfiguration.DataFreshnessInSeconds == 600 (the created value; 600 is also the documented default).
  2. UpdateChannel(ChannelArn, ClusterArn, S3DestinationUpdate={DataFreshnessInSeconds: 900}) answers 200 with channelArn equal to the submitted ChannelArn and a clusterOperationArn that is a kafka cluster-operation ARN (an 'arn:aws:kafka:' ARN whose region, account and cluster/operation ids come from the request and the emulator's own state — the literal ARN of any one run is not a requirement).
  3. An independent DescribeChannel converges to Status 'ACTIVE' AND S3DestinationConfiguration.DataFreshnessInSeconds == 900.
  4. ListChannels(ClusterArn) contains exactly one entry whose ChannelArn equals that channel, with Status 'ACTIVE'.
Actual: none of 1-4 is reached; the check is killed at the 60 s wall-clock ceiling with no response recorded.

Poll cadences read from the check source (AWS-sized, which the 60 s ceiling cannot absorb when a state never settles): _wait_for_cluster_state DescribeClusterV2 at 20 s (budget 2700 s); _wait_for_topic_status DescribeTopic at 5 s; _wait_for_channel_status DescribeChannel at 10 s (budget 600 s); _wait_for_channel_config_converged DescribeChannel at 10 s (budget 300 s), returning only when Status == 'ACTIVE' and DataFreshnessInSeconds == 900, and raising on Status 'FAILED'.

Model facts cited: ChannelStatus enum = CREATING|ACTIVE|UPDATING|DELETING|FAILED|SUSPENDING|SUSPENDED; DescribeChannel output required members = channelArn, channelName, status, destinationType, creationTime, topicConfigurationList; UpdateChannel output members = channelArn (required), clusterOperationArn; S3DestinationUpdate.DataFreshnessInSeconds required, 'Allowed range: 300 to 900'; S3DestinationConfiguration.DataFreshnessInSeconds 'Allowed range: 300 to 900. Default: 600.'

Per-run values in this replay — generated cluster/topic/channel/bucket/role names, ARNs, account id, region, host and port, timestamps — are not requirements.

## Steps to Reproduce
Boot the emulator (kafka service) and run the sequence below, measuring the FIRST probe after each write and the whole sequence under a 60 s wall-clock budget:
1. CreateClusterV2 a Provisioned cluster: InstanceType 'express.m7g.large', 3 AZ-distinct ClientSubnets, a KafkaVersion ListKafkaVersions reports ACTIVE below the 4.x line, NumberOfBrokerNodes 3. Poll DescribeClusterV2 and record whether State reaches 'ACTIVE'.
2. CreateTopic on that cluster (PartitionCount 1, ReplicationFactor 2); poll DescribeTopic and record whether Status reaches 'ACTIVE'. Read its TopicArn.
3. Create an S3 bucket and an IAM role trusted by kafka.amazonaws.com with s3:PutObject/GetBucketLocation/ListBucket on that bucket.
4. CreateChannel with TopicConfigurationList=[{TopicArn, RecordConverter:{ValueConverter:'BYTE_ARRAY'}}] and S3DestinationConfiguration={DataFreshnessInSeconds:600, ServiceExecutionRoleArn:<role>, Storage:{BucketArn, CompressionType:'NONE', StorageClass:'STANDARD', OutputPrefix:'data/'}, DeadLetterQueueS3:{BucketArn, ErrorOutputPrefix:'dlq/'}}. Assert DescribeChannel reaches Status 'ACTIVE' with S3DestinationConfiguration.DataFreshnessInSeconds == 600.
5. UpdateChannel(ChannelArn, ClusterArn, S3DestinationUpdate={DataFreshnessInSeconds:900}). Assert 200, channelArn == the submitted ChannelArn, and clusterOperationArn starting 'arn:aws:kafka:'.
6. Poll an INDEPENDENT DescribeChannel and assert it converges to Status 'ACTIVE' with S3DestinationConfiguration.DataFreshnessInSeconds == 900 (not merely that UpdateChannel echoed something).
7. ListChannels(ClusterArn): assert exactly one entry for that ChannelArn, with Status 'ACTIVE'.
8. Replay kafka.channel-update-mutation-reflected-in-describe and confirm it reaches a pass/fail verdict instead of a bare 60 s timeout.
