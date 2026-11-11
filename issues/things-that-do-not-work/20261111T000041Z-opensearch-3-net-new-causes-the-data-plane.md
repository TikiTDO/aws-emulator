Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh opensearch.managed-domain-strips-index-blocks-write-but-honours-read-only opensearch.disabled-operation-gating-across-readonly-apis
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property|run#14|AssertionError|assert got_interval == "30s", (
Issue-Key: opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh|run#11|AssertionError|assert got_interval == "-1", (
Issue-Key: opensearch.managed-domain-strips-index-blocks-write-but-honours-read-only|run#12|AssertionError|assert write_settings.get("index.blocks.write") == "true", (
Issue-Key: opensearch.disabled-operation-gating-across-readonly-apis|_wait_domain_active#24|Timeout|time.sleep(interval_s)
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# opensearch: 3 net-new causes — the data plane ignores flat_settings=true, serializes index settings with JSON types instead of strings, and the read-only-API gating check never reaches a verdict

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Three net-new root causes in the opensearch emulator, all reached through a domain the emulator itself provisioned and published an endpoint for.

Cause 1 of 3: GET /<index>/_settings?flat_settings=true (and GET /_cluster/settings?flat_settings=true) ignores the flat_settings query parameter and answers in the nested object form.
AWS: recorded behavior only — the domain data plane is the OpenSearch engine REST API and is not described by the opensearch Smithy model. The engine contract is that `flat_settings=true` makes a settings read answer one flat map per index whose keys are the full dotted setting names, i.e. {"<index>":{"settings":{"index.refresh_interval":"-1","index.number_of_shards":"1","index.blocks.write":"true", ...}}}; without the parameter the same read answers the nested tree {"<index>":{"settings":{"index":{"refresh_interval":...}}}}. The status is 200 in both cases; nothing is refused.
Emulator: the response is the nested tree regardless of flat_settings=true, so a lookup of the dotted key returns nothing. Replay output: "index 'sim2real-behav-refresh' reports index.refresh_interval None, expected '-1' ... {'sim2real-behav-refresh': {'settings': {'index': {'number_of_replicas': 0, 'refresh_interval': '-1', 'number_of_shards': 1, ...}}}}" and "the index reports index.refresh_interval None after a 200/acknowledged PUT of '30s', expected '30s': {'sim2real-behav-clustersettings': {'settings': {'index': {'number_of_shards': 1, 'refresh_interval': '30s', ...}}}}". The value written by the preceding PUT is present — only the key form is wrong.
Why: every client that reads settings with flat_settings=true (the form used to assert one specific setting without walking a tree) sees the setting as absent, so it concludes a 200/acknowledged settings PUT had no effect, and index-level configuration can never be verified.
Fix: the domain data-plane REST handler for the _settings route in the opensearch service crate — its query-string handling must select the flat projection, and the projection itself must join the dotted path (including the leading `index.`) for every setting.
Traces: opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh, opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property, opensearch.managed-domain-strips-index-blocks-write-but-honours-read-only.

Cause 2 of 3: index setting values are serialized with their JSON types (booleans, integers) instead of as strings.
AWS: recorded behavior only — the engine stores and returns every index setting as a string, so a settings read reports "index.blocks.write": "true", "index.number_of_shards": "1", "index.refresh_interval": "30s" — the quoted string form, never a JSON boolean or number, whatever type was supplied on the PUT.
Emulator: the settings read answers booleans and numbers. Replay output for the blocks check: {'index': {'number_of_replicas': 0, 'number_of_shards': 1, 'blocks.write': True, 'provided_name': ..., 'uuid': ...}} — `blocks.write` is the JSON literal true and `number_of_shards`/`number_of_replicas` are JSON numbers.
Why: a client comparing a setting against the string AWS returns ("true", "1", "0") never matches, so a correctly applied block or shard count reads as unset or as a different value; this persists even once Cause 1 is fixed, because the flat key would then be present with the wrong value type.
Fix: the settings serializer behind the _settings route (and the state that holds applied index settings) in the opensearch service crate — coerce every setting value to its string representation on the way out.
Traces: opensearch.managed-domain-strips-index-blocks-write-but-honours-read-only, opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property, opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh.

Cause 3 of 3: the read-only-API gating scenario never reaches a verdict — the check exhausts the 60 s harness cap.
AWS: Smithy: StartDomainMaintenance POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance, Action: MaintenanceType one of REBOOT_NODE | RESTART_SEARCH_PROCESS | RESTART_DASHBOARD, NodeId length 10..40, errors BaseException(400), DisabledOperationException(409), ResourceNotFoundException(409), ValidationException(400), output MaintenanceId. On a domain scaled to 3 data nodes the RESTART_SEARCH_PROCESS action becomes eligible and returns a MaintenanceId, which ListDomainMaintenances then reports; on a 1-data-node domain it is refused ResourceNotFoundException.
Emulator: the check makes no assertion at all before the cap: it polls DescribeDomain for Created && !Processing && Endpoint, then retries StartDomainMaintenance while the answer is ResourceNotFoundException. Hypothesis: one of those two states is never reached — either the domain does not settle to Created/!Processing with a published Endpoint promptly, or StartDomainMaintenance keeps answering ResourceNotFoundException after the scale-out to 3 data nodes, so eligibility never arrives. Related open issue 20261109T160051Z-opensearch-a-restart-search-process-maintenance-action-never.md describes a RESTART_SEARCH_PROCESS maintenance action that never progresses, but it carries a different replay key and does not cover this check.
Why: the whole class of "a read of a never-created resource must fail rather than fabricate a record" assertions (GetDomainMaintenanceStatus, GetMigration, DescribeDryRunProgress, ListVpcEndpointsForDomain) is unverifiable, and a client scripting a maintenance action against a 3-node domain hangs instead of getting a MaintenanceId.
Fix: the StartDomainMaintenance handler and the domain-node eligibility state it consults, plus the domain lifecycle state that publishes Processing/Endpoint, in the opensearch service crate.
Traces: opensearch.disabled-operation-gating-across-readonly-apis.

## Evidence
Cause 1 — expected (AWS, flat_settings=true): {"<index>":{"settings":{"index.refresh_interval":"-1", "index.number_of_shards":"1", ...}}}; actual (emulator): {'sim2real-behav-refresh': {'settings': {'index': {'number_of_replicas': 0, 'refresh_interval': '-1', 'number_of_shards': 1, 'provided_name': 'sim2real-behav-refresh', 'uuid': '...'}}}} -> lookup of 'index.refresh_interval' yields None. Same shape for the 30s case: 'the index reports index.refresh_interval None after a 200/acknowledged PUT of 30s'.
Cause 2 — expected: 'index.blocks.write' == "true" (string); actual: {'index': {..., 'blocks.write': True, 'number_of_shards': 1, 'number_of_replicas': 0}} — JSON boolean/numbers instead of strings.
Cause 3 — expected: the scenario completes (1-node RESTART_SEARCH_PROCESS refused ResourceNotFoundException, 3-node start returns a MaintenanceId that ListDomainMaintenances lists); actual: 'Check timed out after 60.0s' with no assertion reached.

## Steps to Reproduce
1. CreateDomain (OpenSearch_2.11, t3.small.search x1, EBS gp3 10 GiB, account-scoped es:* access policy) and wait for Processing=false with a published Endpoint.
2. PUT https://<endpoint>/<index> then PUT https://<endpoint>/<index>/_settings with {"index":{"refresh_interval":"30s"}} (SigV4, service 'es'); confirm 200/acknowledged.
3. GET https://<endpoint>/<index>/_settings?flat_settings=true and read the key "index.refresh_interval" — it is absent (Cause 1); the nested value is a JSON string here but booleans/numbers appear for blocks/shards (Cause 2).
4. PUT {"index":{"blocks.write":true}} on another index, then re-read with flat_settings=true: expect "index.blocks.write":"true".
5. For Cause 3: create a 1-data-node domain, call StartDomainMaintenance(Action=RESTART_SEARCH_PROCESS, NodeId=<a Data NodeId>), then UpdateDomainConfig to 3 data nodes and retry StartDomainMaintenance until it is accepted — it never is within the budget.
