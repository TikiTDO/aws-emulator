Issue-Source: trace-sweep-divergence
Issue-Trace: logs-catalog-describe-configuration-templates
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-catalog-describe-configuration-templates
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]
Issue-Commit: b18a62dd9722f81a5987bef5e25a27694a55571a

# DescribeConfigurationTemplates catalog omits allowedFieldDelimiters, allowedSuffixPathFields and fieldDelimiter and misreports logType, resourceType and allowedOutputFormats (10 causes)

> _Note: this issue was identified against the code base at commit `b18a62dd9722f81a5987bef5e25a27694a55571a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationTemplates

## Description
CloudWatch Logs' vended-log configuration-template catalog, served by DescribeConfigurationTemplates, is the API a client uses to discover what it may put in a CreateDelivery / UpdateDeliveryConfiguration request for a given (service, logType, resourceType, deliveryDestinationType) tuple. The emulator's catalog diverges from AWS's on every page the corpus asks for: whole per-service template sets are absent, several members modelled on ConfigurationTemplate are never emitted at all, and for the templates it does emit the (logType, resourceType, suffixPath, allowedOutputFormats, mandatory) values are wrong.

Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): DescribeConfigurationTemplates -> DescribeConfigurationTemplatesResponse{configurationTemplates: ConfigurationTemplates, nextToken}, paginated (items=configurationTemplates, pageSize=limit). ConfigurationTemplate members: service, logType, resourceType, deliveryDestinationType, defaultDeliveryConfigValues (ConfigurationTemplateDeliveryConfigValues{recordFields, fieldDelimiter, s3DeliveryConfiguration}), allowedFields, allowedOutputFormats, allowedActionForAllowVendedLogsDeliveryForResource, allowedFieldDelimiters, allowedSuffixPathFields, deliverySourceConfiguration, s3TablesIntegration. Every one of these is a modelled member of the structure the operation returns; the model documents allowedFieldDelimiters as "the valid values that a caller can use as field delimiters", allowedSuffixPathFields as "the list of variable fields that can be used in the suffix path" and defaultDeliveryConfigValues.fieldDelimiter as the default used "when the field delimiter is not specified in that operation". The concrete catalog contents (which service/logType/resourceType tuples exist and what each allows) are recorded behavior only — the model does not enumerate them — so the recording is the reference for them.

There is a family of open logs issues about delivery sources and delivery destinations (for example 20261030T193849Z-logs-delivery-destination-policy-delete-and-read-back.md and 20261030T214406Z-logs-cloudfront-logs-putdeliverysource-rejects-an-unvended-logtype-with-an.md), but none of them covers DescribeConfigurationTemplates or the catalog it serves; this is net-new.

Cause 1 of 10 — the aidevops template set is absent.
AWS: DescribeConfigurationTemplates for service "aidevops" returns six templates (resourceType "agent-space" family, deliveryDestinationType S3/CWL/FH), each carrying allowedFieldDelimiters ["\t", " ", ","], allowedSuffixPathFields ['accountid','region','resource_id','yyyy','MM','dd','HH'], allowedOutputFormats ['plain','json','w3c','parquet'], defaultDeliveryConfigValues.fieldDelimiter "" and allowedActionForAllowVendedLogsDeliveryForResource "aidevops:AllowVendedLogDeliveryForResource" (recorded behavior only).
Emulator: HTTP 200, but configurationTemplates[0]..[5] are all absent (compared as `missing (expected {...}, got None)`), i.e. the catalog carries no aidevops entries at those positions.
Why: a client asking which aidevops log types can be vended is told none exist, so it cannot build a CreateDelivery request at all.
Fix: the DescribeConfigurationTemplates handler's static configuration-template catalog.
Traces: logs-catalog-describe-configuration-templates#task#1 (cluster also covers task#6, task#13, task#17, task#21, task#24, task#26).

Cause 2 of 10 — apigateway templates: allowedOutputFormats truncated, allowedFieldDelimiters and the default fieldDelimiter dropped.
AWS: the apigateway EXECUTION_LOGS/stages CWL template carries allowedOutputFormats ['plain','json'], allowedFieldDelimiters ["\t", " ", ","] and defaultDeliveryConfigValues.fieldDelimiter "" (recorded behavior only).
Emulator: HTTP 200 with allowedOutputFormats ['json'] only, no allowedFieldDelimiters member and no defaultDeliveryConfigValues.fieldDelimiter, for configurationTemplates[1] and [2].
Why: a client that validates its requested outputFormat against allowedOutputFormats rejects 'plain' as unsupported, and one that reads allowedFieldDelimiters to pick a legal delimiter for a Plain/W3C/Raw delivery finds no legal value.
Fix: the DescribeConfigurationTemplates handler's catalog entries plus the ConfigurationTemplate serializer, which never emits allowedFieldDelimiters or defaultDeliveryConfigValues.fieldDelimiter.
Traces: logs-catalog-describe-configuration-templates#task#2.

Cause 3 of 10 — autoloop templates: allowedFields mandatory flags inverted and allowedSuffixPathFields missing.
AWS: in the autoloop ERROR_LOGS/subscription S3 template, allowedFields marks resource_arn and event_timestamp mandatory=false, there is no 'resource_id' entry, and the template carries allowedSuffixPathFields ['accountid','region','yyyy','MM','dd','HH'] and allowedFieldDelimiters ["\t", " ", ","] (recorded behavior only).
Emulator: HTTP 200 with resource_arn/event_timestamp mandatory=true, an extra 'resource_id' allowed field, and no allowedSuffixPathFields or allowedFieldDelimiters; 21 of the returned templates diverge.
Why: a client that honours the mandatory flags is forced to send record fields AWS treats as optional, and one that validates a suffixPath against allowedSuffixPathFields has no variable it may use.
Fix: the DescribeConfigurationTemplates handler's catalog entries and the ConfigurationTemplate serializer.
Traces: logs-catalog-describe-configuration-templates#task#3 (cluster also covers task#22).

Cause 4 of 10 — bedrock templates are published under the wrong logType.
AWS: the bedrock agent-alias S3 template with the timestamp/resourceId/traceId/spanId/sessionId/requestId/... field set is logType EVENT_LOGS (recorded behavior only).
Emulator: HTTP 200 and the same field set is published as logType APPLICATION_LOGS; 17 templates in the bedrock page diverge.
Why: a client filtering DescribeConfigurationTemplates by logTypes=[EVENT_LOGS] gets nothing back, and a PutDeliverySource/CreateDelivery built from the returned logType names a log type AWS does not vend for that resource.
Fix: the DescribeConfigurationTemplates handler's catalog entries (logType keying).
Traces: logs-catalog-describe-configuration-templates#task#4.

Cause 5 of 10 — the bedrock-agentcore template set diverges wholesale.
AWS: DescribeConfigurationTemplates for service "bedrock-agentcore" returns 48 templates; the workload-identity APPLICATION_LOGS S3 entry carries allowedSuffixPathFields ['accountid','region','workload_identity_id','yyyy','MM','dd','HH'], allowedFieldDelimiters ["\t", " ", ","], allowedOutputFormats ['plain','json','w3c','parquet'] and a 21-entry defaultDeliveryConfigValues.recordFields list beginning resource_arn, event_timestamp, request_id (recorded behavior only).
Emulator: HTTP 200 with configurationTemplates[0]..[47] all diverging — the entry at each index is not the entry AWS serves there.
Why: AgentCore is the service whose deliveries the corpus actually creates; every default a client derives from this page (record fields, suffix path variables, output formats) is wrong, so the delivery it creates is wrong before it is sent.
Fix: the DescribeConfigurationTemplates handler's static configuration-template catalog.
Traces: logs-catalog-describe-configuration-templates#task#5.

Cause 6 of 10 — the codewhisperer template set is absent.
AWS: service "codewhisperer" has EVENT_LOGS/customization templates for S3, CWL and FH, the S3 one with allowedSuffixPathFields ['accountid','region','customization_id','yyyy','MM','dd','HH'] and defaultDeliveryConfigValues.s3DeliveryConfiguration.suffixPath '{region}/{customization_id}/{yyyy}/{MM}/{dd}/{HH}/' (recorded behavior only).
Emulator: HTTP 200 with configurationTemplates[0]..[2] absent (`got None`).
Why: the whole codewhisperer vended-log surface is undiscoverable.
Fix: the DescribeConfigurationTemplates handler's static configuration-template catalog.
Traces: logs-catalog-describe-configuration-templates#task#7 (cluster also covers task#8, task#9, task#12, task#15, task#16, task#23, task#25, task#28, task#29).

Cause 7 of 10 — eks templates: wrong logType and wrong default suffixPath.
AWS: the eks cluster S3 template whose record fields are resource_arn, event_timestamp, level, stream, message, err, error, controller, ... is logType AUTO_MODE_LOAD_BALANCING_LOGS with defaultDeliveryConfigValues.s3DeliveryConfiguration.suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/' (recorded behavior only).
Emulator: HTTP 200 publishing that field set as logType AUTO_MODE_BLOCK_STORAGE_LOGS with suffixPath '{region}/{cluster_id}/{yyyy}/{MM}/{dd}/{HH}/'; 33 templates in the eks page diverge.
Why: a client that creates an EKS Auto Mode load-balancing delivery cannot find its template, and a delivery created from the returned default writes objects under a different S3 key layout than AWS would.
Fix: the DescribeConfigurationTemplates handler's catalog entries (logType keying and per-template S3 defaults).
Traces: logs-catalog-describe-configuration-templates#task#10.

Cause 8 of 10 — elasticloadbalancing templates: wrong logType and no s3TablesIntegration.
AWS: the elasticloadbalancing loadbalancer S3 template whose allowedFields are time, client_ip, client_port, listener_port, tls_protocol, ... is logType ALB_CONNECTION_LOGS, carries s3TablesIntegration {datasourceName: 'aws_alb', datasourceType: 'connection'} and defaultDeliveryConfigValues.s3DeliveryConfiguration.suffixPath '{region}/{yyyy}/{MM}/{dd}/' (recorded behavior only). s3TablesIntegration is a modelled ConfigurationTemplate member.
Emulator: HTTP 200 publishing that position as logType ALB_ACCESS_LOGS with the access-log field set and no s3TablesIntegration member at all; 12 templates diverge.
Why: ALB connection logs are undiscoverable, and a client wiring an S3 Tables integration has no datasource name/type to use.
Fix: the DescribeConfigurationTemplates handler's catalog entries plus the ConfigurationTemplate serializer (s3TablesIntegration is never emitted).
Traces: logs-catalog-describe-configuration-templates#task#11 (cluster also covers task#20).

Cause 9 of 10 — events templates: wrong resourceType and wrong default suffixPath.
AWS: the events ERROR_LOGS S3 template whose record fields are resource_arn, message_timestamp_ms, subscriber_name, request_id, event_id, invocation_id, message_type, log_level, details, error has resourceType 'subscriber', allowedSuffixPathFields ['accountid','subscriber_name','region','yyyy','MM','dd','HH'] and suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/' (recorded behavior only).
Emulator: HTTP 200 with resourceType 'event-bus' and suffixPath '{region}/{event_bus_name}/{yyyy}/{MM}/{dd}/{HH}/', and resource_arn/message_timestamp_ms marked mandatory=true where AWS marks them false; 18 templates diverge.
Why: a client filtering by resourceTypes=['subscriber'] finds no template, and a delivery built from the returned default uses a suffix-path variable that is not in AWS's allowed set for this log type.
Fix: the DescribeConfigurationTemplates handler's catalog entries (resourceType keying and per-template S3 defaults).
Traces: logs-catalog-describe-configuration-templates#task#14.

Cause 10 of 10 — the mediatailor template set is absent.
AWS: service "mediatailor" returns nine templates, the first with allowedFields timestamp, originId, eventTimestamp (mandatory), awsAccountId (mandatory), customerId, eventType, sessionType, requestId, ... (recorded behavior only).
Emulator: HTTP 200 with configurationTemplates[0]..[8] absent.
Why: the whole MediaTailor vended-log surface is undiscoverable.
Fix: the DescribeConfigurationTemplates handler's static configuration-template catalog.
Traces: logs-catalog-describe-configuration-templates#task#18 (cluster also covers task#19, task#27).

## Evidence
Replay of trace set simparity-wire-aws-v1 (186 fixtures, 1217 steps, 1057 passed). Ten net-new DescribeConfigurationTemplates divergence clusters, all HTTP 200 vs 200 with body mismatches:

1. logs-catalog-describe-configuration-templates#task#1 (7 steps): $.configurationTemplates[0]..[5] missing (expected the aidevops templates with allowedFieldDelimiters ['\t',' ',','], allowedSuffixPathFields ['accountid','region','resource_id','yyyy','MM','dd','HH'], allowedOutputFormats ['plain','json','w3c','parquet'], defaultDeliveryConfigValues.fieldDelimiter ''; got None).
2. #task#2: $.configurationTemplates[1],[2] missing — expected allowedOutputFormats ['plain','json'] + allowedFieldDelimiters + fieldDelimiter '', got allowedOutputFormats ['json'] with neither allowedFieldDelimiters nor fieldDelimiter.
3. #task#3 (also task#22): $.configurationTemplates[0]..[20] — expected autoloop resource_arn/event_timestamp mandatory False and allowedSuffixPathFields present; got mandatory True, extra 'resource_id', no allowedSuffixPathFields/allowedFieldDelimiters.
4. #task#4: $.configurationTemplates[0]..[16] — expected bedrock agent-alias logType 'EVENT_LOGS'; got 'APPLICATION_LOGS'.
5. #task#5: $.configurationTemplates[0]..[47] — expected bedrock-agentcore workload-identity APPLICATION_LOGS template with 21 default recordFields and allowedSuffixPathFields ['accountid','region','workload_identity_id','yyyy','MM','dd','HH'].
6. #task#7 (10 steps): $.configurationTemplates[0]..[2] missing (expected the codewhisperer EVENT_LOGS/customization templates; got None).
7. #task#10: $.configurationTemplates[0]..[32] — expected eks logType 'AUTO_MODE_LOAD_BALANCING_LOGS' with suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'; got 'AUTO_MODE_BLOCK_STORAGE_LOGS' with suffixPath '{region}/{cluster_id}/{yyyy}/{MM}/{dd}/{HH}/'.
8. #task#11 (also task#20): $.configurationTemplates[0]..[11] — expected elasticloadbalancing logType 'ALB_CONNECTION_LOGS' with s3TablesIntegration {datasourceName:'aws_alb', datasourceType:'connection'}; got 'ALB_ACCESS_LOGS' and no s3TablesIntegration.
9. #task#14: $.configurationTemplates[0]..[17] — expected events resourceType 'subscriber' with suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'; got resourceType 'event-bus' with suffixPath '{region}/{event_bus_name}/{yyyy}/{MM}/{dd}/{HH}/'.
10. #task#18 (also task#19, task#27): $.configurationTemplates[0]..[8] missing (expected the mediatailor templates; got None).

Cross-cutting in the emulator's output: allowedFieldDelimiters, allowedSuffixPathFields, s3TablesIntegration and defaultDeliveryConfigValues.fieldDelimiter are never emitted for any template, and allowedOutputFormats is reduced to ['json'] / the S3 cases keep only part of ['plain','json','w3c','parquet'].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture logs-catalog-describe-configuration-templates, step task#1: POST the CloudWatch Logs DescribeConfigurationTemplates target with {"service": "aidevops"} and compare configurationTemplates[0..5] with the recorded response — the emulator returns no matching entries.
3. Repeat for the same fixture's steps task#2 (service apigateway), task#3 (autoloop), task#4 (bedrock), task#5 (bedrock-agentcore), task#7 (codewhisperer), task#10 (eks), task#11 (elasticloadbalancing), task#14 (events) and task#18 (mediatailor).
4. In each response, check for the members allowedFieldDelimiters, allowedSuffixPathFields, s3TablesIntegration and defaultDeliveryConfigValues.fieldDelimiter, and compare logType/resourceType/allowedOutputFormats and the allowedFields mandatory flags against the recording.
