Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-configured-table-analysis-rule-round-trip
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-configured-table-analysis-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|GetConfiguredTableAnalysisRule|4xx->4xx|$.message
Issue-Commit: f4f656b1bd7a513d31ed52598c20b4db3f989ef9

# GetConfiguredTableAnalysisRule not-found message renders the modeled enum value instead of AWS's internal analysis-rule type name

> _Note: this issue was identified against the code base at commit `f4f656b1bd7a513d31ed52598c20b4db3f989ef9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguredTableAnalysisRule

## Description
One net-new root cause on the cleanrooms configured-table analysis-rule read plane. 1 cause must be closed before this issue is submitted.

Cause 1 of 1: when a configured table holds no analysis rule of the requested type, GetConfiguredTableAnalysisRule is refused with the right status (404) and the right code (ResourceNotFoundException), but the rule type interpolated into the message is the MODELED enum value rather than the internal rule-type name AWS renders in that sentence.

AWS: Smithy: GetConfiguredTableAnalysisRule, GET /configuredTables/{configuredTableIdentifier}/analysisRule/{analysisRuleType} (@readonly, success 200, required output member analysisRule); both path members are @required @httpLabel, analysisRuleType targets the enum ConfiguredTableAnalysisRuleType = AGGREGATION | LIST | CUSTOM, configuredTableIdentifier is length 36..36 with pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$; modeled errors AccessDeniedException(403) | InternalServerException(500) | ResourceNotFoundException(404) | ThrottlingException(429) | ValidationException(400), ResourceNotFoundException documented "Request references a resource which does not exist." The model does not state the refusal wording, so recorded behavior is the authority here. Recorded behavior: the miss is refused HTTP 404 with code ResourceNotFoundException and the message template "No such analysis rule with configuredTableId: <configuredTableIdentifier> and type <internalAnalysisRuleTypeName>" — the FIXED text is "No such analysis rule with configuredTableId: " and " and type " (no trailing period); the VARIABLE parts are the configured table id taken verbatim from the request path, and the analysis rule type rendered by its INTERNAL service-side name, which for the modeled member LIST is INTERSECTION_LIST. The same internal-name rendering is recorded on the sibling association read (where the modeled LIST member is reported as "IntersectionList"), so this is the service's general rule for this message, not one recording's quirk; the SUCCESS response is unaffected and still carries analysisRule.type as the modeled value ("LIST").

Emulator: answers 404 ResourceNotFoundException (status and code correct) with "No such analysis rule with configuredTableId: <configuredTableIdentifier> and type LIST" — the fixed text matches, but it substitutes the modeled enum value of the requested analysisRuleType straight into the " and type " slot instead of that type's internal name, so for a LIST request the sentence ends "type LIST" where AWS ends it "type INTERSECTION_LIST". The configured table uuid in the recording is a per-run value and is not part of the requirement; the differing part is only the rule-type token.

Why: a client that matches or parses this refusal (the documented not-found sentence is what error-classification tables, log scrapers and "does this table already have a rule of this type?" probes key on) does not recognise the emulator's sentence, and a client that reads the type back out of the message to decide which rule to create is handed a token AWS never emits in that position — so a probe-then-create flow built against AWS mis-branches against the emulator.

Fix: the shared refusal-message builder for this plane, state::no_such_configured_table_analysis_rule_message (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/state.rs), as called from the GetConfiguredTableAnalysisRule handler's not-found arm — it must map the modeled ConfiguredTableAnalysisRuleType to AWS's internal rule-type name when interpolating the " and type " slot (LIST -> INTERSECTION_LIST) rather than formatting the modeled member, leaving the 404 status, the ResourceNotFoundException code and the resourceId/resourceType members as they are. Related but NOT covering: the open issue 20261029T213946Z-cleanrooms-refusal-fidelity-duplicate-analysis-rule-classified-conflictexception.md records the same internal-name rendering for the sibling GetConfiguredTableAssociationAnalysisRule, but its cause there is that the operation is an unimplemented stub — a different operation and a different defect, so implementing it does not fix this message.

Traces: cleanrooms-configured-table-analysis-rule-round-trip#task#9.

## Evidence
Wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200, data listener :38201 accepting; 72 steps, 46 passed, score 63.9%).

Cause 1 — fixture cleanrooms-configured-table-analysis-rule-round-trip, step task#9, operation GetConfiguredTableAnalysisRule, verdict mismatch, divergence_key service=cleanrooms|GetConfiguredTableAnalysisRule|status=404->404|keys=$.message. Expected status 404, actual status 404 (status and x-amzn-errortype agree). Body divergence: $.message value_mismatch — expected 'No such analysis rule with configuredTableId: 84a81562-e3d7-48ec-b6af-44a2b75e92ad and type INTERSECTION_LIST', got 'No such analysis rule with configuredTableId: 84a81562-e3d7-48ec-b6af-44a2b75e92ad and type LIST'. The configured-table id is identical in both (so the id interpolation is correct); the sole diverging token is the rule type, INTERSECTION_LIST vs LIST. Cluster size 1 (affected: cleanrooms-configured-table-analysis-rule-round-trip#task#9); stub_path empty and owned_by empty, so this failure is net-new and owned by no open issue.

Model evidence (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json): GetConfiguredTableAnalysisRule is @readonly GET /configuredTables/{configuredTableIdentifier}/analysisRule/{analysisRuleType}, success 200, output requires analysisRule; errors AccessDenied(403), InternalServer(500), ResourceNotFound(404), Throttling(429), Validation(400); ConfiguredTableAnalysisRuleType = AGGREGATION | LIST | CUSTOM. The model states no message text for ResourceNotFoundException, hence "recorded behavior only" for the wording.

Code evidence: services/.../src/ops/get_configured_table_analysis_rule.rs lines 177-189 build the 404 via st::no_such_configured_table_analysis_rule_message(&input.configured_table_identifier, &input.analysis_rule_type), passing the modeled enum value straight through to the message builder in src/state.rs, which is why the modeled name surfaces in the sentence.

## Steps to Reproduce
Boot the emulator and replay the wire fixture cleanrooms-configured-table-analysis-rule-round-trip from trace set simparity-wire-aws-v1.

1. Cause 1 — replay fixture cleanrooms-configured-table-analysis-rule-round-trip up to step task#9: create a configured table, then GET /configuredTables/{configuredTableIdentifier}/analysisRule/LIST at a point where that table holds no LIST analysis rule (the round-trip reaches this after deleting the rule it created). Observe HTTP 404 with x-amzn-errortype ResourceNotFoundException and body $.message ending "... and type LIST", where AWS ends the same sentence "... and type INTERSECTION_LIST" for the identical request. The fixed text "No such analysis rule with configuredTableId: " / " and type " and the interpolated configured-table id already match; only the rule-type token diverges.
