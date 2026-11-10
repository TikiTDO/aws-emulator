Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#64
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: add629af7d6117f223747ffc9fa4e155dbf41c42

# schemas ListSchemas omits AWS-managed schemas from the aws.events catalog

> _Note: this issue was identified against the code base at commit `add629af7d6117f223747ffc9fa4e155dbf41c42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
ONE shared fault, 10 numbered causes below: the emulator's seeded catalog of AWS-managed (discovered) schemas for the `aws.events` registry does not contain every schema the registry publishes, so GET /v1/registries/name/aws.events/schemas returns pages that are short or hold different entries than AWS's. All ten causes are the same root fault observed on a different event source; a fixer MUST close all ten (each has its own fixture#step below) before submitting.

SHARED SHAPE
AWS: Smithy `ListSchemas` (GET /v1/registries/name/{RegistryName}/schemas, code 200) returns `Schemas: list<SchemaSummary>` with `SchemaSummary` members SchemaName, LastModified (@timestampFormat date-time), VersionCount, Tags (jsonName `tags`), SchemaArn, and is @paginated {inputToken/outputToken=NextToken, items=Schemas, pageSize=Limit}. Which schemas exist in the `aws.events` registry is not in the model, so this is recorded behavior only: the recording shows one SchemaSummary per published AWS-managed schema, for every event source, enumerated across NextToken pages.
Emulator: for each source below, schemas the recording lists are absent from the response entirely (the replay reports `missing (expected {...}, got None)` for those positions) or the positions are occupied by the next source's schemas because the whole source is missing from the catalog. Status is 200 in both cases; only the body differs.
Why: a client enumerating the discovered-schema registry (to pick a schema for code bindings, to check that an event source is covered, or to drive a UI list) never sees these schemas, and the follow-up DescribeSchema / GetCodeBindingSource for a name it legitimately expected cannot be issued at all. Pagination makes it worse: a short page shifts every later page, so entries that do exist are also skipped.
Fix: the schemas service crate's seeded AWS-managed schema catalog — the registry state that backs ListSchemas for the `aws.events` registry — must carry every published schema name for every event source; the ListSchemas handler then pages over that catalog. This is a state/seed-completeness defect, not a serializer defect: the entries that are served are shaped correctly.
NOT part of this issue: every cluster also reports LastModified differing (the emulator serves one constant sentinel for all entries, the recording carries per-schema values). Those values are recording-run clock values and are deliberately excluded here. Related but NOT covering this: open issue 20261101T214917Z-schemas-aws-managed-schema-content-remains-a-seeded-document.md covers DescribeSchema/ExportSchema `Content`, not ListSchemas catalog membership.

Cause 1 of 10 — aws.ds (fixture schemas-catalog-list-schemas, step task#64; same cluster also task#69, task#80, task#90). The only diverging position in the page: expected $.Schemas[0] = SchemaName 'aws.ds@DirectoryServiceDomainControllerDiscoveryUpdate', VersionCount 1, tags {}; emulator returned None at that position (the page is one entry short).

Cause 2 of 10 — aws.ec2spotfleet (step task#72; also task#109, task#114). The entire event source is absent: all 10 expected entries, $.Schemas[0]..[9] — EC2FleetError, EC2FleetInformation, EC2FleetInstanceChange, EC2FleetSpotInstanceRequestChange, EC2FleetStateChange, EC2SpotFleetError, EC2SpotFleetInformation, EC2SpotFleetInstanceChange, EC2SpotFleetSpotInstanceRequestChange, EC2SpotFleetStateChange — came back None.

Cause 3 of 10 — aws.social-messaging (step task#188). The WhatsApp schema family is absent: 17 positions ($.Schemas[0]..[16]) expected names of the form aws.social-messaging@WhatsAppCall* (Accepted, Connect, Failed, HandoverFailed, HandoverSucceeded, Rejected, Ringing, Terminate) and aws.social-messaging@WhatsAppMessage* (Delivered, Failed, HandoverFailed, ...) and all returned None.

Cause 4 of 10 — aws.dms (step task#60; also task#200). The source's non-CloudTrail schemas are absent from $.Schemas[6] on: expected 'aws.dms@DMSDataMigrationStateChange', 'aws.dms@DMSReplicationEndpointStateChange' and the rest of the family, got None; 32 positions diverge in that page.

Cause 5 of 10 — aws.gamelift (step task#95). Expected $.Schemas[6] = 'aws.gamelift@GameLiftFleetEvent' and the GameLift event family that follows it, got None; 37 positions diverge.

Cause 6 of 10 — aws.ivs (step task#123). Expected $.Schemas[6] = 'aws.ivs@IVSAdBreakStateChange', $.Schemas[7] = 'aws.ivs@IVSChannelAgentSessionStateChange' and the rest of the IVS family, got None; 24 positions diverge.

Cause 7 of 10 — aws.ses (step task#179). Expected $.Schemas[6] = 'aws.ses@AdvisorRecommendationStatusOpen', $.Schemas[7] = 'aws.ses@AdvisorRecommendationStatusResolved' and the rest, got None; 20 positions diverge.

Cause 8 of 10 — aws.ssm (step task#193). Expected $.Schemas[6] = 'aws.ssm@ApproverAccessRequestStatusUpdate', $.Schemas[7] = 'aws.ssm@CalendarStateChange' and the rest of the SSM family, got None; 33 positions diverge.

Cause 9 of 10 — aws.oam (step task#145). The whole aws.oam source is missing, so the page that should begin with 'aws.oam@AWSAPICallviaCloudTrail' (and go on through the aws.oam CloudTrail family and 'aws.oam@AttachedLinkCreated') instead begins with 'aws.opensearch@AWSAPICallviaCloudTrail' and continues into aws.opsworks; 89 positions diverge and no aws.oam schema is served anywhere in the catalog.

Cause 10 of 10 — aws.kafka-cluster (step task#126; also task#144). The whole aws.kafka-cluster source is missing: the page that should begin with 'aws.kafka-cluster@AWSAPICallviaCloudTrail' (through its CloudTrail family) instead begins with 'aws.kafka@AWSAPICallviaCloudTrail'; 53 positions diverge.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture schemas-catalog-list-schemas, operation ListSchemas, expected 200 / actual 200 — body only).

Cause 1, task#64: expected $.Schemas[0] {'SchemaName': 'aws.ds@DirectoryServiceDomainControllerDiscoveryUpdate', 'VersionCount': 1, 'tags': {}} — actual: None (position absent).
Cause 2, task#72: expected $.Schemas[0..9] = the ten aws.ec2spotfleet@EC2Fleet*/EC2SpotFleet* summaries — actual: None for all ten.
Cause 3, task#188: expected $.Schemas[0..16] = aws.social-messaging@WhatsAppCall*/WhatsAppMessage* summaries — actual: None for all seventeen.
Cause 4, task#60: expected $.Schemas[6] 'aws.dms@DMSDataMigrationStateChange', [7] 'aws.dms@DMSReplicationEndpointStateChange' — actual: None.
Cause 5, task#95: expected $.Schemas[6] 'aws.gamelift@GameLiftFleetEvent' — actual: None.
Cause 6, task#123: expected $.Schemas[6] 'aws.ivs@IVSAdBreakStateChange', [7] 'aws.ivs@IVSChannelAgentSessionStateChange' — actual: None.
Cause 7, task#179: expected $.Schemas[6] 'aws.ses@AdvisorRecommendationStatusOpen', [7] 'aws.ses@AdvisorRecommendationStatusResolved' — actual: None.
Cause 8, task#193: expected $.Schemas[6] 'aws.ssm@ApproverAccessRequestStatusUpdate', [7] 'aws.ssm@CalendarStateChange' — actual: None.
Cause 9, task#145: expected $.Schemas[0] 'aws.oam@AWSAPICallviaCloudTrail' ... [6] 'aws.oam@AttachedLinkCreated' — actual $.Schemas[0] 'aws.opensearch@AWSAPICallviaCloudTrail' ... [6] an aws.opsworks entry; no aws.oam schema served.
Cause 10, task#126: expected $.Schemas[0] 'aws.kafka-cluster@AWSAPICallviaCloudTrail' ... [5] 'aws.kafka-cluster@AWSServiceEventviaCloudTrail', [6] 'aws.kafkaconnect@AWSAPICallviaCloudTrail' — actual $.Schemas[0..5] 'aws.kafka@AWS*viaCloudTrail'; no aws.kafka-cluster schema served.

Model cited: ListSchemas http GET /v1/registries/name/{RegistryName}/schemas code 200, output Schemas: __listOfSchemaSummary, @paginated{inputToken=NextToken, outputToken=NextToken, items=Schemas, pageSize=Limit}; SchemaSummary{SchemaName, LastModified(date-time), VersionCount, Tags jsonName=tags, SchemaArn}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `schemas-catalog-list-schemas` from trace set simparity-wire-aws-v1.
2. Inspect step task#64 (representative): GET /v1/registries/name/aws.events/schemas paged with limit/nextToken until the page containing the aws.ds source, and compare $.Schemas against the recording — 'aws.ds@DirectoryServiceDomainControllerDiscoveryUpdate' is absent from the emulator's page.
3. Repeat for steps task#72, task#188, task#60, task#95, task#123, task#179, task#193, task#145 and task#126 for causes 2-10 (aws.ec2spotfleet, aws.social-messaging, aws.dms, aws.gamelift, aws.ivs, aws.ses, aws.ssm, aws.oam, aws.kafka-cluster).
4. Ignore LastModified when comparing: it is a recording-run clock value and is not part of this issue.
