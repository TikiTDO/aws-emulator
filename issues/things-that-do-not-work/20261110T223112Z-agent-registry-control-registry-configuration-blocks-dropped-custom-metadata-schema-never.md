Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-custom-metadata-schema-enforcement
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-custom-metadata-schema-enforcement agent-registry-control-record-token-replay-rename-and-descriptor-errors
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->2xx|$.message
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->2xx|status
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->4xx|$.message
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: agent-registry-control|CreateRegistryRecord|4xx->4xx|status
Issue-Key: agent-registry-control|GetRegistry|2xx->2xx|$.approvalConfiguration
Issue-Key: agent-registry-control|GetRegistry|2xx->2xx|$.createdAt
Issue-Key: agent-registry-control|GetRegistry|2xx->2xx|$.discoveryConfiguration
Issue-Key: agent-registry-control|GetRegistry|2xx->2xx|$.updatedAt
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.customMetadataSchemaComplianceStatus
Issue-Related: 20261104T045821Z-agent-registry-control-agentregistrycontrol-serializes-date-time-timestamps-as-epoch-numbers.md
Issue-Commit: 677ec28d2ee9442908109a9e8e0002b68ccccfcc

# Registry configuration blocks dropped: custom-metadata schema never enforced or reported

> _Note: this issue was identified against the code base at commit `677ec28d2ee9442908109a9e8e0002b68ccccfcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegistryRecord

## Description
The emulator does not keep or act on a registry's configuration structures. The registry's customMetadataSchemaConfiguration is never applied to record writes and its compliance verdict is never reported, and the registry's discoveryConfiguration / approvalConfiguration are not echoed back by GetRegistry. This bundle has FOUR causes; all four must be fixed and re-replayed.

Cause 1 of 4 — CreateRegistryRecord accepts customMetadata that violates the registry's schema (fixture agent-registry-control-custom-metadata-schema-enforcement, step task#3).
AWS: Smithy: CreateRegistryRecord POST /registries/{registryId}/records, success code 202; input member customMetadata -> CustomMetadataDocument (document, @sensitive); the registry carries customMetadataSchemaConfiguration -> CustomMetadataSchemaConfiguration { defaultSchema (JSON Schema for common attributes), recordTypeSchemaOverrides (per-recordType fields) }; modeled error ValidationException (errorKind client, httpError 400, required member message, reason ValidationExceptionReason default FieldValidationFailed). A record whose customMetadata does not satisfy the schema that applies to its recordType is refused with ValidationException: HTTP 400, header x-amzn-errortype: ValidationException. Message template, as recorded: the fixed text "Custom metadata validation failed for record type <recordType>: " followed by the JSON-Schema failure for the offending field; the variable parts are the recordType from the request, the JSON pointer of the offending member from the request's customMetadata, and the constraint text taken from the registry's schema (e.g. the enumeration's member list).
Emulator: returns 202 Accepted and persists the record. No message member and no x-amzn-errortype header at all.
Why: a client gets a success for metadata the real service rejects, so invalid records enter the registry and the client's error path (catching ValidationException and correcting the field) is never exercised. The accepted record also occupies its name, corrupting every later step in the same session (see Cause 2).
Fix: CreateRegistryRecord's input validation — evaluate customMetadata against the owning registry's customMetadataSchemaConfiguration (defaultSchema plus the recordType override) before the record is written, and raise ValidationException.
Traces: agent-registry-control-custom-metadata-schema-enforcement#task#3 (also agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#10).

Cause 2 of 4 — the same operation reports a name conflict instead of the missing-required-property validation failure (fixture agent-registry-control-custom-metadata-schema-enforcement, step task#4).
AWS: Smithy: same operation and the same ValidationException (400). When the request's customMetadata omits a property the applicable schema requires, the refusal is HTTP 400 / x-amzn-errortype: ValidationException with the fixed text "Custom metadata validation failed for record type <recordType>: : required property '<member>' not found" — variable parts are the recordType from the request and the name of the required member taken from the registry's schema.
Emulator: HTTP 409 with x-amzn-errortype: ConflictException and the message "A record with name '<name>' already exists in registry <registryId>" (ConflictException is modeled at httpError 409, but it is the wrong error class for this request).
Why: the client is told its record name is taken when the actual defect is a missing metadata field, so retrying under a new name still fails on the real service. Hypothesis: the conflict is a cascade of Cause 1 — the invalid record accepted at task#3 already holds the name — but the requirement for this step stands on its own: a schema-invalid create must be a 400 ValidationException naming the missing property.
Fix: CreateRegistryRecord's validation must run (and reject) before the name-uniqueness check in its handler.
Traces: agent-registry-control-custom-metadata-schema-enforcement#task#4 (also agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#11).

Cause 3 of 4 — GetRegistry omits discoveryConfiguration and approvalConfiguration (fixture agent-registry-control-custom-metadata-schema-enforcement, step task#7).
AWS: Smithy: GetRegistry GET /registries/{registryId}, 200; output members discoveryConfiguration -> DiscoveryConfiguration { authorizerType (enum RegistryAuthorizerType), authorizerConfiguration } and approvalConfiguration -> ApprovalConfiguration { autoApprovalRules (list, max 10; "When omitted or empty, submitted records require manual review") }. GetRegistry returns the configuration the registry was created or updated with — in the recording, an approvalConfiguration with an empty autoApprovalRules list and a discoveryConfiguration whose authorizerType is the value supplied at create.
Emulator: both members are absent from the 200 body.
Why: a client cannot read back how the registry is configured — code that branches on authorizerType, or that checks whether auto-approval rules exist before expecting a submitted record to be auto-approved, sees nothing and takes the wrong branch. (The same step also shows createdAt/updatedAt serialized as epoch numbers rather than the modeled @timestampFormat date-time; that encoding defect is already open as 20261104T045821Z-agent-registry-control-agentregistrycontrol-serializes-date-time-timestamps-as-epoch-numbers.md and is NOT this cause.)
Fix: registry state (persist the configuration structures supplied to CreateRegistry/UpdateRegistry) plus GetRegistry's serializer so both structures are emitted, including an empty-list autoApprovalRules.
Traces: agent-registry-control-custom-metadata-schema-enforcement#task#7.

Cause 4 of 4 — GetRegistryRecord omits customMetadataSchemaComplianceStatus (fixture agent-registry-control-custom-metadata-schema-enforcement, step task#9).
AWS: Smithy: GetRegistryRecord GET /registries/{registryId}/records/{recordId}, 200; output member customMetadataSchemaComplianceStatus -> enum CustomMetadataSchemaComplianceStatus with values COMPLIANT, NON_COMPLIANT, documented as "Indicates whether a registry record's custom metadata conforms to the registry's current schema" (COMPLIANT = all required fields present and all values match their declared types). RegistryRecordSummary carries the same member for list operations. A record whose metadata satisfies the registry's current schema reads back COMPLIANT.
Emulator: the member is absent from the 200 body.
Why: a client cannot tell whether a record still conforms after the registry's schema changed, so remediation tooling that lists NON_COMPLIANT records finds none and silently does nothing.
Fix: GetRegistryRecord's serializer plus record state — compute the compliance verdict against the registry's current schema and emit the member (also on ListRegistryRecords summaries).
Traces: agent-registry-control-custom-metadata-schema-enforcement#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, /__admin/health 200):

Cause 1 — CreateRegistryRecord, fixture agent-registry-control-custom-metadata-schema-enforcement step task#3: status expected 400, got 202; $.message missing (expected "Custom metadata validation failed for record type CUSTOM: /environment: does not have a value in the enumeration [\"dev\", \"prod\"]", got None); $headers.x-amzn-errortype missing (expected ValidationException-shaped, got None). Cluster also covers agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#10.

Cause 2 — CreateRegistryRecord, step task#4: status expected 400, got 409; $.message expected "Custom metadata validation failed for record type CUSTOM: : required property 'environment' not found", got "A record with name 'sim2real-probe-arc-bad-eefc2de5' already exists in registry 672768aa2f38"; $headers.x-amzn-errortype got 'ConflictException'. Cluster also covers agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#11.

Cause 3 — GetRegistry, step task#7: $.approvalConfiguration missing (expected {'autoApprovalRules': []}, got None); $.discoveryConfiguration missing (expected {'authorizerType': 'AWS_IAM'}, got None).

Cause 4 — GetRegistryRecord, step task#9: $.customMetadataSchemaComplianceStatus missing (expected 'COMPLIANT', got None).

(The createdAt/updatedAt epoch-number mismatches reported alongside causes 3 and 4 belong to the already-open timestamp-encoding issue.)

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Fixture agent-registry-control-custom-metadata-schema-enforcement:
   - CreateRegistry with a customMetadataSchemaConfiguration whose CUSTOM record-type schema requires an 'environment' property constrained to an enumeration, plus a discoveryConfiguration (authorizerType) and an approvalConfiguration with no auto-approval rules.
   - step task#3: CreateRegistryRecord with recordType CUSTOM and customMetadata whose 'environment' value is outside the enumeration -> expect 400 ValidationException, emulator returns 202.
   - step task#4: CreateRegistryRecord with recordType CUSTOM and customMetadata omitting 'environment' -> expect 400 ValidationException naming the missing required property, emulator returns 409 ConflictException about a duplicate name.
   - step task#7: GetRegistry on that registry -> expect discoveryConfiguration and approvalConfiguration in the body, emulator omits both.
   - step task#9: GetRegistryRecord on a schema-conforming record -> expect customMetadataSchemaComplianceStatus COMPLIANT, emulator omits the member.
