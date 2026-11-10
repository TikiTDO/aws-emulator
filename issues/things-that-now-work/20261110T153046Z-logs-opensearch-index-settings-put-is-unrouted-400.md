Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected
Issue-Key: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups|run#37|AssertionError|assert status_code == 200, (
Issue-Key: logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected|run#15|AssertionError|assert refused == want_refusal, (
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# logs__opensearch: index _settings PUT is unrouted (400 'no handler found'), and CreateDomain's nonexistent-log-group refusal carries the resource-policy message (2 causes)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 2: the OpenSearch domain's index settings endpoint does not serve PUT, so slow-log thresholds cannot be configured.
AWS: PUT /<index>/_settings on a domain's search endpoint updates the index's dynamic settings (e.g. index.search.slowlog.threshold.query.warn / index.indexing.slowlog.threshold.index.warn) and answers 200 with {"acknowledged": true}; the configured thresholds then drive slow-log records into the log groups named by the domain's LogPublishingOptions. Recorded behavior only (the engine REST surface is not in the Smithy model).
Emulator: the request is refused 400 with {'error': 'no handler found for uri [/<index>/_settings] and method [PUT]'} — the URI/method pair is not routed at all.
Why: slow-log thresholds can never be set, so SEARCH_SLOW_LOGS/INDEX_SLOW_LOGS publishing is unreachable and a client's index-settings management fails with a transport-level 'no handler' error rather than any modelled refusal.
Fix: the opensearch domain engine's REST router — add the index _settings PUT handler (and have it feed the slow-log thresholds the log-publishing path reads).
Traces: logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups.

Cause 2 of 2: CreateDomain with LogPublishingOptions naming a log group that does not exist is refused with the wrong message.
AWS: when the named CloudWatch Logs log group does not exist, CreateDomain answers 400 ValidationException with the message "The specified CloudWatch Logs log group doesn't exist." — fixed text, no variable part. The resource-access-policy sentence ('The Resource Access Policy specified for the CloudWatch Logs log group <name> does not grant sufficient permissions for Amazon OpenSearch Service to create a log stream. Please add the resource access policy using the PutResourcePolicy API in the CloudWatch Logs service.') is the refusal for a group that EXISTS but is not granted, and its only variable part is the group name. Smithy opensearch CreateDomain errors: ValidationException @httpError 400, BaseException 400, DisabledOperationException 409, InvalidTypeException 409, LimitExceededException 409, ResourceAlreadyExistsException 409, InternalException 500.
Emulator: it answers ValidationException with the resource-access-policy sentence for a group that does not exist at all, i.e. it checks the policy before existence and reports the wrong cause.
Why: a customer is told to add a resource policy to a log group that is not there, so the actual fix (create the group) is never indicated; the two distinct failure modes become indistinguishable.
Fix: the opensearch CreateDomain LogPublishingOptions validation — assert log-group existence first and emit the existence message, keeping the policy message for an existing-but-ungranted group.
Traces: logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected.

## Evidence
Cause 1 — expected 200 on the slowlog threshold update; actual: 'AssertionError: expected the slowlog threshold update on <index> to be accepted, got 400: {\'error\': \'no handler found for uri [/<index>/_settings] and method [PUT]\'}'.
Cause 2 — expected ('ValidationException', "The specified CloudWatch Logs log group doesn't exist."); actual ('ValidationException', 'The Resource Access Policy specified for the CloudWatch Logs log group <group> does not grant sufficient permissions for Amazon OpenSearch Service to create a log stream. Please add the resource access policy using the PutResourcePolicy API in the CloudWatch Logs service.'). The group name in the actual message is a per-run value; the wrong sentence is the defect.

## Steps to Reproduce
Cause 1: replay logs__opensearch.obs-opensearch-3-index-slow-and-application-logs-publish-to-their-own-log-groups — create a domain with SEARCH_SLOW_LOGS/INDEX_SLOW_LOGS/ES_APPLICATION_LOGS publishing, create an index and PUT /<index>/_settings with slowlog thresholds. Cause 2: replay logs__opensearch.osdocstat-log-publishing-to-nonexistent-group-at-create-rejected — CreateDomain with LogPublishingOptions whose CloudWatchLogsLogGroupArn names a log group that was never created, and inspect the refusal code and message.
