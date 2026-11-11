Issue-Source: trace-sweep-divergence
Issue-Trace: es-application-defaults-and-insights-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-application-defaults-and-insights-errors opensearch-application-default-setting-lifecycle opensearch-application-lifecycle-workflow opensearch-unset-default-application es-cross-cluster-connection-errors es-domain-mutation-not-found-errors es-domain-read-not-found-errors opensearch-domain-lifecycle-and-config-inspection esval-ebs-gp3-volume-size-below-instance-floor esval-ebs-magnetic-volume-size-above-cap esval-range-offpeak-window-hours-24 esval-range-offpeak-window-minutes-60 opensearch-domain-creation-and-deletion-errors es-index-and-domain-datasource-errors esdef-application-dead-id-probe opensearch-application-capability-lifecycle opensearch-domain-insights-operations opensearch-insight-feedback-thumbs-up-domain-not-found opensearch-insight-feedback-thumbs-up-with-text opensearch-domain-version-capacity-and-software-update-management opensearch-vpc-endpoint-directory opensearch-vpc-endpoint-error-paths
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|GetDefaultApplicationSetting|4xx->4xx|status
Issue-Key: es|AcceptInboundConnection|4xx->4xx|status
Issue-Key: es|RejectInboundConnection|4xx->4xx|status
Issue-Key: es|DeleteInboundConnection|4xx->4xx|status
Issue-Key: es|DeleteOutboundConnection|4xx->4xx|status
Issue-Key: es|UpdateDomainConfig|4xx->4xx|status
Issue-Key: es|CancelDomainConfigChange|4xx->4xx|status
Issue-Key: es|StartServiceSoftwareUpdate|4xx->4xx|status
Issue-Key: es|CancelServiceSoftwareUpdate|4xx->4xx|status
Issue-Key: es|RollbackServiceSoftwareUpdate|4xx->4xx|status
Issue-Key: es|UpgradeDomain|4xx->4xx|status
Issue-Key: es|UpdateScheduledAction|4xx->4xx|status
Issue-Key: es|DescribeDomain|4xx->4xx|status
Issue-Key: es|DescribeDomainConfig|4xx->4xx|status
Issue-Key: es|DescribeDomainAutoTunes|4xx->4xx|status
Issue-Key: es|ListScheduledActions|4xx->4xx|status
Issue-Key: es|GetUpgradeHistory|4xx->4xx|status
Issue-Key: es|GetUpgradeStatus|4xx->4xx|status
Issue-Key: es|DeleteDomain|4xx->4xx|status
Issue-Key: es|GetIndex|4xx->4xx|status
Issue-Key: es|DeleteIndex|4xx->4xx|status
Issue-Key: es|ListDataSources|4xx->4xx|status
Issue-Key: es|GetApplication|4xx->4xx|$
Issue-Key: es|GetApplication|4xx->4xx|status
Issue-Key: es|UpdateApplication|4xx->4xx|status
Issue-Key: es|InsightFeedback|4xx->4xx|status
Issue-Key: es|GetCompatibleVersions|4xx->4xx|status
Issue-Key: es|DeleteVpcEndpoint|4xx->4xx|status
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: ResourceNotFoundException answered as HTTP 409 instead of 404 across 28 operations

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
ONE root cause, reproduced by 28 divergence clusters covering 28 distinct operations.

Cause 1 of 1: the HTTP status the emulator binds to ResourceNotFoundException is wrong for the Amazon OpenSearch Service / Elasticsearch Service wire.

AWS: every recorded not-found refusal on this service answers HTTP 404 with header x-amzn-errortype: ResourceNotFoundException and a JSON body carrying __type and message. The checked-in model disagrees with the wire here and must not be used as the source of the status: Smithy (com.amazonaws.opensearch) declares ResourceNotFoundException with @httpError(409); the recorded responses for the same error shape are 404. Recorded behavior is the requirement. The error code and the message text are unchanged by this cause - only the status line is wrong.

Emulator: the same refusals are serialized with status 409 (the modeled httpError), e.g. GetApplication on a deleted application returns 409 with body {"__type": "ResourceNotFoundException", "message": "Application does not exist"} where the recording is 404 with the same message. One fixture (esdef-application-dead-id-probe#task#4) polls for the 404 and therefore burns its entire 60 s / 61-attempt budget before failing.

Why: an SDK or client that branches on the HTTP status (404 -> treat as absent, 409 -> treat as a conflict and retry/abort) takes the wrong branch for every absent domain, application, index, data source, connection, VPC endpoint, scheduled action and upgrade record. Delete-if-exists and wait-until-gone loops never terminate; conflict-retry loops retry a permanent condition.

Fix: the service's error-to-HTTP-status mapping (the layer that serializes a ResourceNotFoundException into a response), not the individual handlers - all 28 operations already raise the right error shape with the right message, so a single mapping change closes all of them.

Traces: representative es-application-defaults-and-insights-errors#task#2 (GetDefaultApplicationSetting, expected 404 got 409). The other 27 clusters are listed in trace_ids and span AcceptInboundConnection, RejectInboundConnection, DeleteInboundConnection, DeleteOutboundConnection, UpdateDomainConfig, CancelDomainConfigChange, StartServiceSoftwareUpdate, CancelServiceSoftwareUpdate, RollbackServiceSoftwareUpdate, UpgradeDomain, UpdateScheduledAction, DescribeDomain, DescribeDomainConfig, DescribeDomainAutoTunes, ListScheduledActions, GetUpgradeHistory, GetUpgradeStatus, DeleteDomain, GetIndex, DeleteIndex, ListDataSources, GetApplication, UpdateApplication, InsightFeedback, GetCompatibleVersions and DeleteVpcEndpoint.

## Evidence
Expected (recorded AWS): HTTP 404, x-amzn-errortype: ResourceNotFoundException.
Actual (emulator): HTTP 409, x-amzn-errortype: ResourceNotFoundException.
replay detail for the representative: "status: expected 404, got 409" (divergence_key service=es|GetDefaultApplicationSetting|status=404->409|keys=status).
For esdef-application-dead-id-probe#task#4 the detail also records "poll: status 409 not in [404] after 61 attempt(s)" with the body {'__type': 'ResourceNotFoundException', 'message': 'Application does not exist'} vs the expected {'message': 'Application does not exist'} - the message matches, only the status diverges.
Smithy cited: com.amazonaws.opensearch#ResourceNotFoundException httpError=409 (model), contradicted by the recordings' 404.

## Steps to Reproduce
1. Replay fixture es-application-defaults-and-insights-errors, step task#2: GET /2021-01-01/opensearch/application/<absent-id>/defaultSetting style read against an absent resource.
2. Observe HTTP 409 where the recording is HTTP 404.
3. Repeat with any trace_ids entry, e.g. es-domain-read-not-found-errors#task#1 (DescribeDomain on sim2real-probe-absent) or opensearch-vpc-endpoint-directory#task#2 (DeleteVpcEndpoint on an absent endpoint).
