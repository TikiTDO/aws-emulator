Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-delivery-source-destination-pipeline
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-delivery-source-destination-pipeline logs-delivery-destination-and-cross-account-policy-lifecycle logs-get-delivery-destination-policy-returns-empty-when-unset logs-end-to-end-delivery-pipeline-create-configure-update
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|$.__type
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|$.message
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|status
Issue-Key: logs|GetDeliveryDestinationPolicy|2xx->2xx|$.policy
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->4xx|$.message
Issue-Key: logs|GetDeliveryDestinationPolicy|2xx->2xx|$
Issue-Key: logs|CreateDelivery|2xx->2xx|$.delivery.fieldDelimiter
Issue-Key: logs|CreateDelivery|2xx->2xx|$.delivery.recordFields
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.fieldDelimiter
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.recordFields
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.s3DeliveryConfiguration
Issue-Key: logs|UpdateDeliveryConfiguration|4xx->2xx|$
Issue-Key: logs|UpdateDeliveryConfiguration|4xx->2xx|status
Issue-Key: logs|DescribeDeliveries|4xx->4xx|$.message
Issue-Related: 20261029T222444Z-logs-describeconfigurationtemplates-serves-an-incomplete-synthesized-vended-logs.md
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs vended-delivery pipeline: delivery records and destination policies lose modeled fields and validation (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
Ten root causes in the vended-log delivery pipeline: the delivery-destination policy surface accepts and echoes documents AWS refuses, Delivery records come back without the members AWS always populates, and two refusal texts are emulator-authored. Cause count: 10 — all ten must be closed. The open issue 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md covers other delivery-plumbing gaps and carries different replay keys; none of the causes below is covered by it.

=== Cause 1 of 10: PutDeliveryDestinationPolicy accepts a policy document that is not a valid resource policy ===
AWS: refuses with HTTP 400 ValidationException and the fixed text 'Invalid Delivery Destination resource Policy is provided.' (no variable parts). Smithy: PutDeliveryDestinationPolicy binds ValidationException (httpError 400) and constrains deliveryDestinationPolicy to @length 1..51200 only, so the document's semantic validity is a handler check.
Emulator: answers HTTP 200 with a success body and stores the document.
Why: a client that relies on the refusal to detect a malformed cross-account grant believes the grant is in place; the destination then silently has a policy AWS would never have accepted.
Fix: the deliveryDestinationPolicy validation branch of the PutDeliveryDestinationPolicy handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-b2-delivery-source-destination-pipeline#task#8.

=== Cause 2 of 10: GetDeliveryDestinationPolicy serves the policy that the refused put should never have stored ===
AWS: after the refusal in cause 1 the destination still has no policy, so GetDeliveryDestinationPolicy returns policy as an EMPTY structure ({}).
Emulator: returns policy.deliveryDestinationPolicy containing the rejected document.
Why: the read path confirms a cross-account permission that does not exist on AWS — the worst possible outcome for a permissions probe.
Fix: the same validation branch (nothing must be persisted) plus the GetDeliveryDestinationPolicy read path.
Traces: logs-b2-delivery-source-destination-pipeline#task#9.

=== Cause 3 of 10: PutDeliveryDestinationPolicy's refusal text is emulator-authored ===
AWS: when the document is not a valid delivery-destination policy the ValidationException message is the fixed text 'Invalid Delivery Destination resource Policy is provided.'
Emulator: 'deliveryDestinationPolicy must be an IAM policy document (a JSON object)'.
Why: the status and code are right but a client matching AWS's sentence does not recognise the refusal.
Fix: the message in that validation branch.
Traces: logs-delivery-destination-and-cross-account-policy-lifecycle#task#7.

=== Cause 4 of 10: GetDeliveryDestinationPolicy omits the `policy` member when no policy is set ===
AWS: for a destination with no policy the response body still carries the member — {'policy': {}} — because Smithy models the output as policy: Policy (a structure with the single optional member deliveryDestinationPolicy).
Emulator: returns an entirely empty body {}.
Why: SDKs and clients that dereference response.policy to test for a policy get a null/absent structure instead of an empty one and either throw or report 'unknown'.
Fix: the GetDeliveryDestinationPolicy serializer — always emit `policy`.
Traces: logs-delivery-destination-and-cross-account-policy-lifecycle#task#9.

=== Cause 5 of 10: DescribeConfigurationTemplates slice for service=qbusiness is a synthesized placeholder ===
AWS: returns the qbusiness templates with the real chat field family (conversation_id, system_message_id, user_message, output_metrics_*, usefulness*, chat_latency, time_to_first_token, plugin_id, ...), allowedFieldDelimiters ['\t',' ',','], allowedOutputFormats ['plain','json','w3c','parquet'], allowedSuffixPathFields ['accountid','application_id','region','yyyy','MM','dd','HH'] and defaultDeliveryConfigValues with the ordered recordFields plus an s3DeliveryConfiguration.
Emulator: returns an entry that does not match on any of those members.
Why: a delivery configured from this template specifies record fields that are not the qbusiness log schema.
Fix: the static catalog behind DescribeConfigurationTemplates (same component as the catalog issue filed separately for the other service slices).
Traces: logs-b2-delivery-source-destination-pipeline#task#2.

=== Cause 6 of 10: DescribeConfigurationTemplates slice for service=securityhub is a synthesized placeholder ===
AWS: two templates, logType SECURITY_FINDING_LOGS on resourceTypes 'hubv2' and 'hub', each with a single mandatory field 'event', allowedOutputFormats ['plain'], allowedFieldDelimiters ['\t',' ',','] and defaultDeliveryConfigValues {fieldDelimiter '', recordFields ['event']}.
Emulator: one entry only (resourceType 'hubv2'), whose allowedFields are the five @-prefixed Insights fields, allowedOutputFormats ['json'] and recordFields the same five fields; the 'hub' entry is missing.
Why: the end-to-end delivery fixture builds its CreateDelivery from this template, so every downstream field selection is wrong.
Fix: the same static catalog.
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#setup#3.

=== Cause 7 of 10: CreateDelivery's returned Delivery omits recordFields and fieldDelimiter ===
AWS: CreateDelivery echoes the effective delivery configuration — the Delivery structure carries recordFields (defaulted from the matching configuration template's defaultDeliveryConfigValues.recordFields when the request omits them) and fieldDelimiter (the template default, the empty string for these templates). Smithy: Delivery.recordFields (RecordFields, @length 0..128) and Delivery.fieldDelimiter (@length 0..5).
Emulator: both members are absent from delivery.
Why: a client that reads back the delivery to learn which fields will be delivered sees nothing and cannot distinguish 'defaulted' from 'not configured'.
Fix: the CreateDelivery handler — resolve the template defaults into the stored Delivery and serialize them.
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#task#5.

=== Cause 8 of 10: GetDelivery omits recordFields, fieldDelimiter and s3DeliveryConfiguration ===
AWS: GetDelivery returns the same fully-resolved Delivery, including s3DeliveryConfiguration {enableHiveCompatiblePath, suffixPath} even when they take their defaults.
Emulator: all three members absent.
Why: the delivery's effective configuration is unreadable, so a configure-then-verify workflow cannot confirm its own change.
Fix: the Delivery state record and the GetDelivery serializer (same defaulting as cause 7).
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#task#6.

=== Cause 9 of 10: UpdateDeliveryConfiguration accepts a recordFields selection the delivery source does not support ===
AWS: refuses with HTTP 400 ValidationException and the fixed text 'Record fields selection is not supported by this Delivery Source.' Smithy: UpdateDeliveryConfiguration binds ValidationException (httpError 400).
Emulator: answers HTTP 200 with an empty body and applies the update.
Why: the client believes it narrowed the delivered fields; on AWS the call fails, so the two environments diverge permanently from this point in the workflow.
Fix: the record-field support check in the UpdateDeliveryConfiguration handler (consult the delivery source's configuration template).
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#task#8.

=== Cause 10 of 10: DescribeDeliveries' invalid-token refusal text is emulator-authored ===
AWS: a malformed/unknown nextToken is refused with HTTP 400 and the fixed text 'Invalid Next Token.' (no variable parts).
Emulator: 'nextToken is not a valid token'.
Why: paginating clients that key on AWS's sentence to restart from the first page instead surface an unrecognised error.
Fix: the pagination-token decoder's error message, shared by the delivery list operations.
Traces: logs-end-to-end-delivery-pipeline-create-configure-update#task#11.

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException', got None); $.message: missing (expected 'Invalid Delivery Destination resource Policy is provided.', got None)".
Cause 2: "$.policy: value_mismatch (expected {}, got {'deliveryDestinationPolicy': '{\"Version\": \"2012-10-17\", \"Statement\": [...]}'})" — the stored document's ARN/account values are per-run; the requirement is that `policy` is empty.
Cause 3: "$.message: value_mismatch (expected 'Invalid Delivery Destination resource Policy is provided.', got 'deliveryDestinationPolicy must be an IAM policy document (a JSON object)')" at HTTP 400 -> 400.
Cause 4: "$: empty_body (expected {'policy': {}}, got {})" at HTTP 200 -> 200.
Cause 5: "$.configurationTemplates[0]: value_mismatch (expected {'allowedActionForAllowVendedLogsDeliveryForResource': 'qbusiness:AllowVendedLogDeliveryForResource', 'allowedFieldDelimiters': ['\t', ' ', ','], 'allowedFields': [... 'conversation_id', 'system_message_id', 'user_message', 'output_metrics_is_message_blocked', ...], 'allowedOutputFormats': ['plain', 'json', 'w3c', 'parquet'], 'allowedSuffixPathFields': ['accountid', 'application_id', 'region', 'yyyy', 'MM', 'dd', 'HH'], ...})".
Cause 6: "$.configurationTemplates[0]: missing (expected {... 'allowedFields': [{'mandatory': True, 'name': 'event'}], 'allowedOutputFormats': ['plain'], 'defaultDeliveryConfigValues': {'fieldDelimiter': '', 'recordFields': ['event']}, 'deliveryDestinationType': 'CWL', 'logType': 'SECURITY_FINDING_LOGS', 'resourceType': 'hubv2'}, got {... 'allowedFields': [{'name': '@ingestionTime', ...}, {'name': '@timestamp', 'mandatory': True}], 'allowedOutputFormats': ['json']}); $.configurationTemplates[1]: missing (expected the resourceType 'hub' entry, got None)".
Cause 7: "$.delivery.fieldDelimiter: missing (expected '', got None); $.delivery.recordFields: missing (expected ['event'], got None)".
Cause 8: "$.delivery.fieldDelimiter: missing (expected '', got None); $.delivery.recordFields: missing (expected ['event'], got None); $.delivery.s3DeliveryConfiguration: missing (expected {'enableHiveCompatiblePath': False, 'suffixPath': ''}, got None)".
Cause 9: "status: expected 400, got 200; $: empty_body (expected {'__type': 'ValidationException', 'message': 'Record fields selection is not supported by this Delivery Source.'}, got {})".
Cause 10: "$.message: value_mismatch (expected 'Invalid Next Token.', got 'nextToken is not a valid token')" at HTTP 400 -> 400.

## Steps to Reproduce
1+2+3: PutDeliveryDestination, then PutDeliveryDestinationPolicy with a deliveryDestinationPolicy that is not a valid resource policy — expect 400 ValidationException 'Invalid Delivery Destination resource Policy is provided.'; then GetDeliveryDestinationPolicy and expect policy == {} (replay logs-b2-delivery-source-destination-pipeline#task#8, #task#9, logs-delivery-destination-and-cross-account-policy-lifecycle#task#7).
4: GetDeliveryDestinationPolicy for a destination with no policy — expect body {'policy': {}} (logs-delivery-destination-and-cross-account-policy-lifecycle#task#9).
5: DescribeConfigurationTemplates(service='qbusiness') (logs-b2-delivery-source-destination-pipeline#task#2).
6: DescribeConfigurationTemplates(service='securityhub') — expect two entries, resourceTypes 'hubv2' and 'hub' (logs-end-to-end-delivery-pipeline-create-configure-update#setup#3).
7+8: PutDeliverySource + PutDeliveryDestination + CreateDelivery with no recordFields/fieldDelimiter, then GetDelivery — both must echo the template-defaulted recordFields, fieldDelimiter and s3DeliveryConfiguration (#task#5, #task#6).
9: UpdateDeliveryConfiguration on that delivery with a recordFields list — expect 400 ValidationException 'Record fields selection is not supported by this Delivery Source.' (#task#8).
10: DescribeDeliveries(nextToken='<garbage>') — expect 400 with message 'Invalid Next Token.' (#task#11).

## Resolution

Causes 1, 2, 3, 4, 7, 8, 9 and 10 are FIXED, and every replay key this report carries is
`fixed`: `logs-delivery-destination-and-cross-account-policy-lifecycle` and
`logs-get-delivery-destination-policy-returns-empty-when-unset` replay clean.

- 1+2+3: a `deliveryDestinationPolicy` is accepted only where each of its statements is a grant
  of log delivery — it names the delivery resources it covers, allows only actions that act on
  those resources, and grants them to an account — so a policy for some other resource is
  refused with the fixed text and nothing is stored
  (`src/state/delivery_destination_policy.rs`). The read of a destination that was never given
  one reports an empty `policy` (cause 4, which was already correct).
- 7+8: a delivery's effective configuration is resolved when it is created, from the
  configuration template that governs it, so `recordFields`, `fieldDelimiter` and
  `s3DeliveryConfiguration` are reported by `CreateDelivery`, `GetDelivery` and
  `DescribeDeliveries` alike (`src/state/delivery_configuration.rs`).
- 9: a `recordFields` selection is refused where the delivery source publishes no field a
  delivery may drop, by `UpdateDeliveryConfiguration` and by `CreateDelivery`.
- 10: the delivery enumerations word an unusable `nextToken` as AWS words it.

Cause 5 (the qbusiness slice) and the packaging/membership half of cause 6 are DECLINED here:
the parts of cause 6 this report's own keys depend on are fixed (the catalogue publishes
SECURITY_FINDING_LOGS for both hub resource types, with the single mandatory `event` field, and
every template's default `fieldDelimiter` is the empty string), but which rows the catalogue
holds for each of the twenty service slices, and the `allowedOutputFormats` /
`allowedFieldDelimiters` each row publishes, are one rule across the whole catalogue, owned by
the open issue `20261030T142954Z-logs-describeconfigurationtemplates-catalog-rows-diverge-for-ten-services.md`
with the `logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]` key — which
is not among this report's keys. Those two causes are therefore NOT closed by this change: the
catalogue issue closes them.
