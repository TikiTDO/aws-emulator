Issue-Source: trace-sweep-divergence
Issue-Trace: s3-analytics-configuration-full-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-analytics-configuration-full-lifecycle s3-bucket-monitoring-and-analytics-configurations s3-intelligent-tiering-archive-tier-configuration s3-bucket-lifecycle-notifications-replication s3-bucket-default-encryption-config s3-bucket-default-encryption-setup s3-bucket-encryption-lifecycle s3-notification-config-sqs-filter-roundtrip
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|2xx->2xx|$.ListIntelligentTieringConfigurationsResult
Issue-Key: s3|ListBucketIntelligentTieringConfigurations|2xx->2xx|$.ListIntelligentTieringConfigurationsResult.IntelligentTieringConfiguration
Issue-Key: s3|GetBucketReplication|4xx->2xx|$.Error
Issue-Key: s3|GetBucketReplication|4xx->2xx|status
Issue-Key: s3|GetBucketEncryption|2xx->2xx|$.ServerSideEncryptionConfiguration.Rule.ApplyServerSideEncryptionByDefault.SSEAlgorithm
Issue-Key: s3|GetBucketEncryption|2xx->2xx|$.ServerSideEncryptionConfiguration.Rule.BucketKeyEnabled
Issue-Key: s3|GetBucketNotificationConfiguration|2xx->2xx|$.NotificationConfiguration.QueueConfiguration.Filter.S3Key.FilterRule[*]
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 bucket configuration reads do not reflect what was written

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBucketAnalyticsConfigurations

## Description
Eight net-new root causes sharing one fault: a bucket configuration that was accepted on the write path is not visible, or is visibly altered, on the read path. Two List* operations return nothing at all, one Get* returns a stale success instead of the documented not-found, encryption reads hand back a different algorithm than was written, and notification filter rule names come back in the wrong case. A fixer must close all eight.

Why (common): these are round-trip failures. A client installs a configuration, reads it back to confirm, and either sees nothing (so it re-installs forever, or its convergence poll times out) or sees a different configuration than it wrote. Anything built on read-after-write of bucket configuration — drift detection, IaC refresh, console-style listings — is broken.

Cause 1 of 8: ListBucketAnalyticsConfigurations returns no result element after analytics configurations were accepted.
AWS: the response is <ListBucketAnalyticsConfigurationResult> with IsTruncated and one <AnalyticsConfiguration> per stored configuration. Smithy: ListBucketAnalyticsConfigurationsOutput has xmlName ListBucketAnalyticsConfigurationResult and AnalyticsConfigurationList flattened as AnalyticsConfiguration.
Emulator: the result element is absent (body carries nothing for it) even though PutBucketAnalyticsConfiguration answered success.
Fix: the ListBucketAnalyticsConfigurations handler and the analytics-configuration state the Put path writes.
Traces: s3-analytics-configuration-full-lifecycle#task#6 (also s3-bucket-monitoring-and-analytics-configurations#task#4).

Cause 2 of 8: the same listing is still empty after one of two configurations is deleted, so the surviving one is never visible; the harness polled 58 times.
Emulator: 0 items where 1 was expected.
Fix: as cause 1 — the delete path must leave the remaining configuration listable.
Traces: s3-analytics-configuration-full-lifecycle#task#9.

Cause 3 of 8: ListBucketIntelligentTieringConfigurations returns no result element after a tiering configuration was accepted.
Emulator: <ListIntelligentTieringConfigurationsResult> absent.
Fix: the ListBucketIntelligentTieringConfigurations handler and the state PutBucketIntelligentTieringConfiguration writes.
Traces: s3-bucket-monitoring-and-analytics-configurations#task#19 (also s3-intelligent-tiering-archive-tier-configuration#task#7).

Cause 4 of 8: the same listing is still empty after a second tiering configuration is written and one removed; polled 58 times with 0 items.
Fix: as cause 3, covering the multi-configuration and delete paths.
Traces: s3-intelligent-tiering-archive-tier-configuration#task#10.

Cause 5 of 8: GetBucketReplication answers 200 after the replication configuration was removed.
AWS: 404 with Code 'ReplicationConfigurationNotFoundError', message 'The replication configuration was not found' (fixed text) and <BucketName> naming the bucket.
Emulator: 200 with no Error element.
Fix: the GetBucketReplication handler's not-configured path.
Traces: s3-bucket-lifecycle-notifications-replication#task#12.

Cause 6 of 8: GetBucketEncryption reports aws:kms for a bucket whose default encryption is SSE-S3.
AWS: ServerSideEncryptionConfiguration.Rule.ApplyServerSideEncryptionByDefault.SSEAlgorithm reads back exactly what was configured — 'AES256' for SSE-S3.
Emulator: 'aws:kms'.
Fix: the GetBucketEncryption serializer and the bucket-encryption state (the default must not be rewritten to KMS).
Traces: s3-bucket-default-encryption-config#teardown#10.

Cause 7 of 8: GetBucketEncryption also reports BucketKeyEnabled true for a bucket configured with it false (alongside the same AES256 -> aws:kms substitution).
AWS: Rule.BucketKeyEnabled reads back the configured value.
Emulator: 'true' where 'false' was written.
Fix: as cause 6, including BucketKeyEnabled round-tripping.
Traces: s3-bucket-default-encryption-setup#task#7 (also s3-bucket-encryption-lifecycle#task#5).

Cause 8 of 8: GetBucketNotificationConfiguration lower-cases the S3Key filter rule names.
AWS: the FilterRule Name is echoed capitalised — 'Prefix' and 'Suffix' — which is the form the AWS SDKs emit and parse. (Smithy's FilterRuleName enum lists 'prefix'/'suffix'; the recorded wire form is the capitalised one, so this is recorded behavior.)
Emulator: 'prefix' and 'suffix'.
Why: an SDK or client that matches the rule name exactly cannot find its own prefix/suffix filter after reading the configuration back.
Fix: the GetBucketNotificationConfiguration serializer's FilterRule Name casing.
Traces: s3-notification-config-sqs-filter-roundtrip#task#6

## Evidence
All eight clusters are 'mismatch'.
s3-analytics-configuration-full-lifecycle#task#6: $.ListBucketAnalyticsConfigurationsResult missing (expected IsTruncated 'false' and two AnalyticsConfiguration entries, got None)
s3-analytics-configuration-full-lifecycle#task#9: result missing and $.…AnalyticsConfiguration poll: 0 item(s), not 1 after 58 attempt(s)
s3-bucket-monitoring-and-analytics-configurations#task#19: $.ListIntelligentTieringConfigurationsResult missing (expected IsTruncated 'false' and one IntelligentTieringConfiguration, got None)
s3-intelligent-tiering-archive-tier-configuration#task#10: result missing and poll: 0 item(s), not 1 after 58 attempt(s)
s3-bucket-lifecycle-notifications-replication#task#12 (GetBucketReplication): status expected 404, got 200; $.Error missing (expected Code 'ReplicationConfigurationNotFoundError', message 'The replication configuration was not found', BucketName)
s3-bucket-default-encryption-config#teardown#10 (GetBucketEncryption): …SSEAlgorithm value_mismatch (expected 'AES256', got 'aws:kms')
s3-bucket-default-encryption-setup#task#7 (GetBucketEncryption): …BucketKeyEnabled value_mismatch (expected 'false', got 'true'); …SSEAlgorithm value_mismatch (expected 'AES256', got 'aws:kms')
s3-notification-config-sqs-filter-roundtrip#task#6: FilterRule[0] expected {'Name': 'Prefix', 'Value': …} got {'Name': 'prefix', …}; FilterRule[1] expected {'Name': 'Suffix', …} got {'Name': 'suffix', …}

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: PutBucketAnalyticsConfiguration twice then GET /<bucket>?analytics with no id; PutBucketIntelligentTieringConfiguration then GET /<bucket>?intelligent-tiering with no id; delete one of each and list again; PutBucketReplication then DeleteBucketReplication then GetBucketReplication; PutBucketEncryption with SSEAlgorithm AES256 and BucketKeyEnabled false then GetBucketEncryption; PutBucketNotificationConfiguration with an S3Key filter carrying prefix and suffix rules then GetBucketNotificationConfiguration.
