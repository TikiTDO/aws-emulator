Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold
Issue-Key: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups|run#52|Timeout|time.sleep(2)
Issue-Key: logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold|run#43|Timeout|time.sleep(2)
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# logs__opensearch: a domain with log publishing enabled delivers no log events to its CloudWatch Logs group, so both slow-log/application-log delivery checks reach no verdict

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainConfig

## Description
Cause 1 of 1: an OpenSearch domain whose LogPublishingOptions are enabled never writes any log event into the named CloudWatch Logs group, so both delivery checks hang until the 60 s harness cap and grade nothing.
AWS: Smithy: LogPublishingOption { CloudWatchLogsLogGroupArn (length 20..2048), Enabled: boolean }; LogType enum = INDEX_SLOW_LOGS | SEARCH_SLOW_LOGS | ES_APPLICATION_LOGS | AUDIT_LOGS. The documented behavior (opensearch-service developer guide, createdomain-configure-slow-logs) is that once a log type is enabled on a domain whose target group carries a resource policy granting es.amazonaws.com logs:CreateLogStream + logs:PutLogEvents, the service creates log streams in that group and publishes events to it: an index/search operation on an index whose slowlog threshold is exceeded ('0ms' makes every operation exceed it) produces a slow-log event in the group for that log type, an index whose threshold is '-1' produces none, and each log type lands only in its own group. Delivery is asynchronous but observable; the control-plane calls themselves are 200.
Emulator: neither check reaches any assertion about absence-vs-presence; each exhausts the suite's 60 s cap while polling the log group for a delivered event (reported as 'Check timed out after 60.0s'). Hypothesis: LogPublishingOptions are accepted and echoed by the control plane but no publishing path exists, so the awaited event never appears and the poll cannot terminate.
Why: a client that enables slow logs or application logs to diagnose a domain sees an empty log group forever, and the differential contracts the feature exists for (over-threshold vs '-1' index, slow-log group vs application-log group) cannot be observed at all.
Fix: the OpenSearch domain's log-publishing path in the opensearch service crate — the index/search/application log emitters that must call into CloudWatch Logs (CreateLogStream + PutLogEvents) for each enabled LogType, and the per-index slowlog threshold state that gates them. Two already-open issues (20261102T090835Z-logs-opensearch-opensearch-createdomain-never-validates-logpublishingoptions-the-legacy-es.md and 20261029T182036Z-logs-opensearch-createdomain-accepts-logpublishingoptions-naming-a-log.md) cover VALIDATION of LogPublishingOptions on CreateDomain; neither covers delivery of events.
Traces: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups, logs__opensearch.opensearch-search-slow-logs-reach-the-log-group-only-for-an-index-over-threshold.

## Evidence
Both checks: 'Error: Check timed out after 60.0s' with no assertion reached. Expected: after a disabled->enabled UpdateDomainConfig (or a create with SEARCH_SLOW_LOGS enabled) and an index whose threshold key (index.search.slowlog.threshold.query.warn / index.indexing.slowlog.threshold.index.warn) is '0ms', GetLogEvents on the configured group returns at least one slow-log event for the operation, and zero events for the twin index whose threshold is '-1'; the INDEX_SLOW_LOGS line never appears in the ES_APPLICATION_LOGS group. Actual: no event ever appears, so the poll never terminates.

## Steps to Reproduce
1. CreateLogGroup and PutResourcePolicy (resource-scoped) granting es.amazonaws.com logs:CreateLogStream + logs:PutLogEvents on <group-arn>:*.
2. CreateDomain (OpenSearch_2.11, single t3.small.search node) with SEARCH_SLOW_LOGS publishing to that group — or create with none and then UpdateDomainConfig to enable INDEX_SLOW_LOGS and ES_APPLICATION_LOGS to two distinct groups; wait for Active.
3. PUT /<index>/_settings with index.search.slowlog.threshold.query.warn='0ms' on one index and '-1' on a twin.
4. Run a signed search/index operation against each index.
5. Poll DescribeLogStreams/GetLogEvents on each group: AWS yields a slow-log event for the 0ms index only; the emulator yields nothing and the check burns its budget.
