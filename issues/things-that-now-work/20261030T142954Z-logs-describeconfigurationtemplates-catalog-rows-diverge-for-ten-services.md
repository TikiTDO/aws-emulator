Issue-Source: trace-sweep-divergence
Issue-Trace: logs-catalog-describe-configuration-templates
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-catalog-describe-configuration-templates
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]
Issue-Commit: 6d14a9a831b2c017b564e34b5138bca4659f1a33

# logs DescribeConfigurationTemplates catalog rows diverge for ten services

> _Note: this issue was identified against the code base at commit `6d14a9a831b2c017b564e34b5138bca4659f1a33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationTemplates

## Description
CloudWatch Logs DescribeConfigurationTemplates is a read-only catalog reader: it returns the vended-log configuration templates AWS publishes for each (service, logType, resourceType, deliveryDestinationType) tuple. The emulator serves an emulator-authored catalog instead of the AWS one. The shared fault has one shape, repeated per catalog slice: the emulator has only CWL-destination rows, so every S3 row AWS publishes is absent; and the rows it does serve drop the S3-only members of ConfigurationTemplate (allowedFieldDelimiters, allowedSuffixPathFields, defaultDeliveryConfigValues.fieldDelimiter, defaultDeliveryConfigValues.s3DeliveryConfiguration, s3TablesIntegration), collapse allowedOutputFormats to ["json"], and carry different recordFields and different allowedFields.mandatory flags than AWS. Ten clusters, one per catalog slice the corpus queries.

Smithy used throughout (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): DescribeConfigurationTemplates input {service, logTypes, resourceTypes, deliveryDestinationTypes, nextToken, limit}, output {configurationTemplates: ConfigurationTemplates, nextToken}; shape ConfigurationTemplate members service/logType/resourceType/deliveryDestinationType/defaultDeliveryConfigValues/allowedFields/allowedOutputFormats/allowedActionForAllowVendedLogsDeliveryForResource/allowedFieldDelimiters/allowedSuffixPathFields/deliverySourceConfiguration/s3TablesIntegration; enum DeliveryDestinationType values [S3, CWL, FH, XRAY]; ConfigurationTemplateDeliveryConfigValues members recordFields/fieldDelimiter/s3DeliveryConfiguration. The model names the members but does not enumerate catalog contents, so the per-row content below is recorded behavior only.

Cause 1 of 10: the aidevops slice returns no rows at all.
AWS: recorded behavior only — the queried slice returns a non-empty configurationTemplates list, its first row service 'aidevops', deliveryDestinationType 'S3', allowedActionForAllowVendedLogsDeliveryForResource 'aidevops:AllowVendedLogDeliveryForResource', allowedFieldDelimiters ["\t", " ", ","], allowedOutputFormats ['plain','json','w3c','parquet'], allowedSuffixPathFields ['accountid','region','resource_id','yyyy','MM','dd','HH'], defaultDeliveryConfigValues.fieldDelimiter '' and defaultDeliveryConfigValues.s3DeliveryConfiguration present.
Emulator: status 200 but configurationTemplates[0]..[5] are all missing (got None) — the emulator's catalog has no row matching this slice.
Why: a client enumerating which vended-log templates exist for this source sees an empty catalog and concludes the log type cannot be delivered, so it never calls CreateDelivery.
Fix: the DescribeConfigurationTemplates handler's static configuration-template catalog in the cloudwatch-logs service crate (catalog data, not the serializer).
Traces: logs-catalog-describe-configuration-templates#task#1 (also task#6, task#13, task#17, task#21, task#24).

Cause 2 of 10: the apigateway EXECUTION_LOGS/stages slice serves a CWL row where AWS serves an S3 row.
AWS: recorded behavior only — row {service 'apigateway', logType 'EXECUTION_LOGS', resourceType 'stages', deliveryDestinationType 'S3', allowedFieldDelimiters ["\t", " ", ","], allowedOutputFormats ['plain','json','w3c','parquet'], allowedSuffixPathFields ['accountid','region','api_id','stage','yyyy','MM','dd','HH'], defaultDeliveryConfigValues {fieldDelimiter '', recordFields ['resource_arn','event_timestamp','api_id','stage','resource_path','http_method','payload'], s3DeliveryConfiguration {enableHiveCompatiblePath false, suffixPath '{region}/{api_id}/{stage}/'}}, allowedFields with resource_arn and event_timestamp mandatory false}.
Emulator: deliveryDestinationType 'CWL'; allowedFieldDelimiters, allowedSuffixPathFields, fieldDelimiter and s3DeliveryConfiguration absent; allowedOutputFormats ['json']; recordFields prefixed with 'timestamp','resource_arn','resource_id','event_timestamp'; allowedFields marks resource_arn and event_timestamp mandatory true.
Why: a client that reads the template to build a CreateDelivery call for an S3 destination gets no suffixPath/fieldDelimiter defaults, believes only json output is allowed, and treats optional record fields as mandatory.
Fix: same static catalog; also the row's S3 variant and the S3-only members' population.
Traces: logs-catalog-describe-configuration-templates#task#2 (also task#7, task#8, task#9, task#12, task#15, task#16, task#23, task#25, task#26, task#28, task#29).

Cause 3 of 10: the autoloop ERROR_LOGS/subscription slice.
AWS: recorded behavior only — row deliveryDestinationType 'S3' with allowedFieldDelimiters, allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'], allowedOutputFormats ['plain','json','w3c','parquet'], defaultDeliveryConfigValues {fieldDelimiter '', recordFields ['resource_arn','event_timestamp','log_timestamp','transformer_arn','object_identifier','event_identifier','trace_identifier','log_message'], s3DeliveryConfiguration {enableHiveCompatiblePath false, suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'}}.
Emulator: deliveryDestinationType 'CWL', S3-only members absent, recordFields include 'timestamp' and 'resource_id', allowedFields.mandatory differs; 21 rows of the slice diverge.
Why: same as cause 2 — the S3 delivery path for this source is undiscoverable.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#3 (also task#22).

Cause 4 of 10: the bedrock agent-alias slice reports the wrong logType.
AWS: recorded behavior only — row {service 'bedrock', logType 'EVENT_LOGS', resourceType 'agent-alias', deliveryDestinationType 'S3', allowedSuffixPathFields ['accountid','region','resource_id','yyyy','MM','dd','HH'], s3DeliveryConfiguration {enableHiveCompatiblePath false, suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'}}.
Emulator: the row at that position carries logType 'APPLICATION_LOGS' and deliveryDestinationType 'CWL'; the EVENT_LOGS row is absent and 17 rows of the slice diverge.
Why: a client filtering by logTypes=['EVENT_LOGS'] finds nothing and cannot configure bedrock event-log delivery.
Fix: same static catalog (row identity: service/logType/resourceType/deliveryDestinationType keys).
Traces: logs-catalog-describe-configuration-templates#task#4.

Cause 5 of 10: the bedrock-agentcore workload-identity slice.
AWS: recorded behavior only — row {service 'bedrock-agentcore', logType 'APPLICATION_LOGS', resourceType 'workload-identity', deliveryDestinationType 'S3', allowedFieldDelimiters, allowedSuffixPathFields ['accountid','region','workload_identity_id','yyyy','MM','dd','HH'], allowedOutputFormats ['plain','json','w3c','parquet'], fieldDelimiter '', s3DeliveryConfiguration present}.
Emulator: the expected row is missing and 48 rows of the returned slice diverge from the recorded catalog.
Why: the agentcore delivery catalog a client enumerates is wrong in both membership and defaults.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#5.

Cause 6 of 10: the eks cluster slice reports the wrong logType and drops the S3 row.
AWS: recorded behavior only — row {service 'eks', logType 'AUTO_MODE_LOAD_BALANCING_LOGS', resourceType 'cluster', deliveryDestinationType 'S3', allowedSuffixPathFields ['accountid','cluster_id','region','yyyy','MM','dd','HH'], recordFields ['resource_arn','event_timestamp','level','stream','message','err','error','controller','controllerGroup','controllerKind','reconcileID','worker-count','attempt','delay','totalItems']}.
Emulator: that position holds logType 'AUTO_MODE_BLOCK_STORAGE_LOGS', deliveryDestinationType 'CWL', a different recordFields list, and 33 rows of the slice diverge.
Why: EKS auto-mode load-balancing logs appear unsupported, and the cluster_id suffix-path field is undiscoverable.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#10.

Cause 7 of 10: the elasticloadbalancing loadbalancer slice drops ALB_CONNECTION_LOGS and its s3TablesIntegration.
AWS: recorded behavior only — row {service 'elasticloadbalancing', logType 'ALB_CONNECTION_LOGS', resourceType 'loadbalancer', deliveryDestinationType 'S3', s3TablesIntegration {datasourceName 'aws_alb', datasourceType 'connection'}, allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'], s3DeliveryConfiguration {enableHiveCompatiblePath false, suffixPath '{region}/{yyyy}/{MM}/{dd}/'}}.
Emulator: that position holds logType 'ALB_ACCESS_LOGS' with deliveryDestinationType 'CWL' and no s3TablesIntegration member at all; 12 rows of the slice diverge.
Why: a client cannot discover ALB connection logs, and the modeled s3TablesIntegration member is never populated anywhere in the catalog, so S3-Tables-backed delivery looks unavailable.
Fix: same static catalog, plus population of the ConfigurationTemplate.s3TablesIntegration member.
Traces: logs-catalog-describe-configuration-templates#task#11 (also task#20).

Cause 8 of 10: the events slice reports resourceType 'event-bus' where AWS reports 'subscriber'.
AWS: recorded behavior only — row {service 'events', logType 'ERROR_LOGS', resourceType 'subscriber', deliveryDestinationType 'S3', allowedSuffixPathFields ['accountid','subscriber_name','region','yyyy','MM','dd','HH'], recordFields ['resource_arn','message_timestamp_ms','subscriber_name','request_id','event_id','invocation_id','message_type','log_level','details','error']}.
Emulator: resourceType 'event-bus', deliveryDestinationType 'CWL', recordFields using 'event_timestamp'/'payload' instead of 'message_timestamp_ms'/'details'/'error'; 18 rows of the slice diverge.
Why: a client filtering by resourceTypes=['subscriber'] gets nothing, and the record-field vocabulary it would send in CreateDelivery is wrong.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#14.

Cause 9 of 10: the mediatailor slice.
AWS: recorded behavior only — row {service 'mediatailor', deliveryDestinationType 'S3', allowedFieldDelimiters ["\t", " ", ","], allowedOutputFormats ['plain','json','w3c','parquet'], allowedFields including eventTimestamp and awsAccountId mandatory true and a long list of ad-decision fields}.
Emulator: the expected row is missing; 9 rows of the slice diverge.
Why: the mediatailor vended-log template a client reads before CreateDelivery is not the AWS one.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#18 (also task#19, task#27).

Cause 10 of 10: the workmail organization slice.
AWS: recorded behavior only — row {service 'workmail', logType 'ACCESS_CONTROL_LOGS', resourceType 'organization', deliveryDestinationType 'S3', allowedFieldDelimiters, allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'], allowedOutputFormats ['plain','json','w3c','parquet'], recordFields ['organization_arn','event_timestamp','request_id','user_id','impersonator_id','protocol','source_ip','user_agent','scope','rule_id','access_granted'], s3DeliveryConfiguration present, and allowedFields marking request_id/user_id/protocol/source_ip/scope/access_granted mandatory true}.
Emulator: only a CWL row, allowedOutputFormats ['json'], recordFields missing request_id/impersonator_id/user_agent/rule_id, mandatory flags false, no S3-only members; 15 rows of the slice diverge.
Why: the workmail access-control-log delivery contract a client builds from the template is incomplete and mis-typed.
Fix: same static catalog.
Traces: logs-catalog-describe-configuration-templates#task#30.

No open issue owns these: the replay report lists owned_by=[] for all ten clusters. The nearest open issues concern other logs surfaces (20261029T222444Z-logs-vended-delivery-pipeline-delivery-records-and-destination.md covers delivery records and destination policies, not the configuration-template catalog) and the same class of defect in other services (e.g. 20261029T185333Z-eks-read-only-catalogs-serve-incomplete-static-data.md).

## Evidence
Replay of trace_set simparity-wire-aws-v1 against the emulator built from this checkout; all ten clusters are verdict=mismatch, expected_status 200 / actual_status 200, so the divergence is purely response content.

Cause 1 (logs-catalog-describe-configuration-templates#task#1): detail '$.configurationTemplates[0]: missing (expected {... "service": "aidevops", "deliveryDestinationType": "S3", "allowedFieldDelimiters": ["\t", " ", ","], "allowedSuffixPathFields": ["accountid","region","resource_id","yyyy","MM","dd","HH"] ...}, got None)'; divergence_key service=logs|DescribeConfigurationTemplates|status=200->200|keys=$.configurationTemplates[0]..[5].

Cause 2 (…#task#2): expected '"deliveryDestinationType": "S3", "allowedOutputFormats": ["plain","json","w3c","parquet"], "defaultDeliveryConfigValues": {"fieldDelimiter": "", "recordFields": ["resource_arn","event_timestamp","api_id","stage","resource_path","http_method","payload"], "s3DeliveryConfiguration": {"enableHiveCompatiblePath": false, "suffixPath": "{region}/{api_id}/{stage}/"}}'; got '"deliveryDestinationType": "CWL", "allowedOutputFormats": ["json"], "defaultDeliveryConfigValues": {"recordFields": ["timestamp","resource_arn","resource_id","event_timestamp","api_id","stage","resource_path","http_method","payload"]}' with resource_arn/event_timestamp mandatory true. 12 affected steps.

Cause 3 (…#task#3): expected S3 row with suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'; got CWL row, S3 members absent; keys $.configurationTemplates[0]..[20].

Cause 4 (…#task#4): expected logType 'EVENT_LOGS' / 'S3'; got logType 'APPLICATION_LOGS' / 'CWL'; keys [0]..[16].

Cause 5 (…#task#5): expected service 'bedrock-agentcore' S3 row with allowedSuffixPathFields ['accountid','region','workload_identity_id','yyyy','MM','dd','HH']; got None; keys [0]..[47].

Cause 6 (…#task#10): expected logType 'AUTO_MODE_LOAD_BALANCING_LOGS' / 'S3' with allowedSuffixPathFields ['accountid','cluster_id','region','yyyy','MM','dd','HH']; got logType 'AUTO_MODE_BLOCK_STORAGE_LOGS' / 'CWL'; keys [0]..[32].

Cause 7 (…#task#11): expected logType 'ALB_CONNECTION_LOGS' / 'S3' with s3TablesIntegration {'datasourceName': 'aws_alb', 'datasourceType': 'connection'}; got logType 'ALB_ACCESS_LOGS' / 'CWL' with no s3TablesIntegration; keys [0]..[11]; 2 affected steps.

Cause 8 (…#task#14): expected resourceType 'subscriber' / 'S3' with recordFields ['resource_arn','message_timestamp_ms','subscriber_name',...]; got resourceType 'event-bus' / 'CWL' with recordFields ['timestamp','resource_arn','resource_id','event_timestamp','subscriber_name',...]; keys [0]..[17].

Cause 9 (…#task#18): expected mediatailor S3 row with allowedOutputFormats ['plain','json','w3c','parquet'] and eventTimestamp/awsAccountId mandatory true; got None; keys [0]..[8]; 3 affected steps.

Cause 10 (…#task#30): expected workmail ACCESS_CONTROL_LOGS 'S3' row with recordFields ['organization_arn','event_timestamp','request_id','user_id','impersonator_id','protocol','source_ip','user_agent','scope','rule_id','access_granted']; got CWL row with recordFields ['timestamp','organization_arn','event_timestamp','user_id','protocol','source_ip','scope','access_granted'] and allowedOutputFormats ['json']; keys [0]..[14].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace_set simparity-wire-aws-v1).
3. Fixture logs-catalog-describe-configuration-templates, step task#1 (representative) and steps task#2, task#3, task#4, task#5, task#10, task#11, task#14, task#18, task#30 (the other nine causes) each issue DescribeConfigurationTemplates with a service/logTypes/resourceTypes/deliveryDestinationTypes filter.
4. Compare each response's configurationTemplates list against the recorded one: the emulator returns 200 but every row is CWL-only, omits allowedFieldDelimiters / allowedSuffixPathFields / defaultDeliveryConfigValues.fieldDelimiter / defaultDeliveryConfigValues.s3DeliveryConfiguration / s3TablesIntegration, collapses allowedOutputFormats to ["json"], and carries different recordFields, logType/resourceType keys and allowedFields.mandatory flags.
5. Re-replay those ten fixture#step bindings to confirm a fix.

## Resolution (what was fixed, and the residue that was not)

Fixed, over the WHOLE catalogue rather than the cited rows:

* a log type's templates are published one per delivery destination type, in the order the
  model's `DeliveryDestinationType` enum declares them (`[S3, CWL, FH, XRAY]`), so every slice
  opens with the S3 row; the X-Ray destination receives `TRACES` alone, while every log type —
  traces included — is deliverable to each destination that receives records;
* `defaultDeliveryConfigValues.recordFields` is the subset of `allowedFields` a delivery carries
  when it names none (the allowed fields in order, less the record's own `timestamp` and the
  sending resource's bare `resource_id`, and never less a mandatory field);
* `defaultDeliveryConfigValues.fieldDelimiter` is the empty delimiter (`FieldDelimiter` is
  modelled `@length 0..5`), not the first allowed one;
* `allowedSuffixPathFields` and the default `s3DeliveryConfiguration.suffixPath` are published
  per sender, including senders that name no resource variable and senders that partition by day;
* the record schemas and `mandatory` flags of apigateway, autoloop, eks, events and workmail
  access-control logs, the Bedrock agent-alias `TRACES` row, and the AgentCore `payments` and
  `tools` rows the `logType` catalogue names.

Residue, still diverging on replay (the key
`logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]`): the recorded row
COUNT of the aidevops (6), autoloop (21), eks (31) and events (18) slices, and the catalogue's
internal row order WITHIN a service (the report states only each slice's first row). Both are
recorded catalogue content that neither the model's `logType` catalogue enumerates nor this
report states, so no rows were invented to reach a count. A later report that carries those rows
should reopen this one.
