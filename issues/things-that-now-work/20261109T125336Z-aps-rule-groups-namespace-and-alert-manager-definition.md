Issue-Source: trace-sweep-divergence
Issue-Trace: amp-rule-groups-and-alert-manager-definition-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amp-rule-groups-and-alert-manager-definition-round-trip
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: aps|DescribeRuleGroupsNamespace|2xx->2xx|$.ruleGroupsNamespace.createdAt
Issue-Key: aps|DescribeRuleGroupsNamespace|2xx->2xx|$.ruleGroupsNamespace.modifiedAt
Issue-Key: aps|DescribeRuleGroupsNamespace|2xx->2xx|$.ruleGroupsNamespace.status.statusReason
Issue-Key: aps|ListRuleGroupsNamespaces|2xx->2xx|$.ruleGroupsNamespaces[*].createdAt
Issue-Key: aps|ListRuleGroupsNamespaces|2xx->2xx|$.ruleGroupsNamespaces[*].modifiedAt
Issue-Key: aps|ListRuleGroupsNamespaces|2xx->2xx|$.ruleGroupsNamespaces[*].status.statusReason
Issue-Key: aps|DescribeAlertManagerDefinition|2xx->2xx|$.alertManagerDefinition.createdAt
Issue-Key: aps|DescribeAlertManagerDefinition|2xx->2xx|$.alertManagerDefinition.modifiedAt
Issue-Key: aps|DescribeAlertManagerDefinition|2xx->2xx|$.alertManagerDefinition.status.statusReason
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# aps rule-groups namespace and alert manager definition read paths drop statusReason and serialize sub-millisecond timestamps (3 causes)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRuleGroupsNamespace

## Description
THREE net-new wire root causes in the aps (Amazon Managed Service for Prometheus) emulator, found by replaying the wire fixture corpus simparity-wire-aws-v1. aps IS served (emulator_up=true, bootstrap_signal=false, 42/49 replayed steps pass), so these are fidelity defects, not bootstrap gaps. All three are 200 OK responses whose BODY diverges, and they share one fault: the projection that turns a persisted rule-groups-namespace / alert-manager-definition record into its modeled description or summary envelope drops the status structure's statusReason member and serializes the record's creation/modification instants straight off the system clock, so the envelope the service publishes and the envelope the emulator publishes are not the same document. THREE causes must be closed before submitting; each cause names its own failing fixture and step.

Related open issue 20261029T223357Z-aps-emulator-authored-refusal-payloads-and-default-scraper.md covers six OTHER aps wire causes (refusal payloads on the scraper/anomaly-detector/workspace paths and the default scraper document) and owns the four other clusters in this replay; it says nothing about the rule-groups-namespace or alert-manager-definition success envelopes, so none of the three causes below is covered there.

Cause 1 of 3: DescribeRuleGroupsNamespace's description envelope omits status.statusReason and reports createdAt/modifiedAt with sub-millisecond precision.
  AWS: Smithy: DescribeRuleGroupsNamespace (GET /workspaces/{workspaceId}/rulegroupsnamespaces/{name}, success 200, @readonly) returns required ruleGroupsNamespace (RuleGroupsNamespaceDescription). That shape requires arn, name, status (RuleGroupsNamespaceStatus), data, createdAt and modifiedAt; RuleGroupsNamespaceStatus has required statusCode and optional statusReason ("The reason for the failure, if any"), and createdAt/modifiedAt target smithy.api#Timestamp, which under restJson1 serializes as an epoch-seconds number. The model does not state whether statusReason is emitted when nothing has failed, nor what sub-second resolution the service uses — recorded behavior only: the recorded response carries status.statusReason PRESENT as the empty string alongside statusCode, and carries createdAt/modifiedAt as epoch-seconds numbers whose SHAPE the corpus asserts (the instant itself is per-run and is not asserted). The emulator's numbers fail that shape assertion. Hypothesis: the shape that fails is the sub-second precision — the service's epoch-seconds encoding carries at most millisecond resolution (three fractional digits), while the emulator emits the clock's full remainder (seven fractional digits, e.g. a value ending .9825149).
  Emulator: answers 200 with the right arn/name/data/statusCode, but $.ruleGroupsNamespace.status.statusReason is ABSENT (the stored record's Option<String> is None and the serializer omits the member rather than emitting the empty string), and $.ruleGroupsNamespace.createdAt / $.ruleGroupsNamespace.modifiedAt are raw clock floats with seven fractional digits.
  Why: a client that reads status.statusReason unconditionally (the member is modeled, and the recorded service always sends it on this envelope) sees an absent key where the service sends a value, so a strongly-typed deserializer that distinguishes absent from empty, or any assertion/diff over the full envelope, takes a different branch against the emulator than against AWS. The over-precise timestamp is a different number shape than any real response carries, so a consumer that round-trips or re-serializes these instants, or compares them for equality with a value obtained elsewhere, cannot reproduce what AWS would have given it.
  Fix: the response projection for RuleGroupsNamespaceDescription in DescribeRuleGroupsNamespace's handler (its status structure must carry the no-failure statusReason the service sends, not None) together with the record's timestamp stamping in the aps service state (the creation/modification instants must be truncated to the resolution the service publishes at the point they are stamped, so every read path reports the same truncated value).
  Traces: amp-rule-groups-and-alert-manager-definition-round-trip#task#4

Cause 2 of 3: ListRuleGroupsNamespaces' summary entries have the SAME two defects, so the listing and the describe disagree with the service in the same way.
  AWS: Smithy: ListRuleGroupsNamespaces (GET /workspaces/{workspaceId}/rulegroupsnamespaces, 200, @readonly, @paginated over ruleGroupsNamespaces) returns required ruleGroupsNamespaces (RuleGroupsNamespaceSummaryList). RuleGroupsNamespaceSummary requires arn, name, status, createdAt and modifiedAt, and reuses the SAME RuleGroupsNamespaceStatus (required statusCode, optional statusReason) as the description shape. Recorded behavior only: each recorded summary carries status.statusReason present as the empty string, and createdAt/modifiedAt as epoch-seconds numbers of the shape the corpus asserts.
  Emulator: each entry of $.ruleGroupsNamespaces omits status.statusReason and carries createdAt/modifiedAt as full-precision clock floats — i.e. the listing serializes the same divergent summary as the describe path.
  Why: the listing is the path a client uses to enumerate namespaces before describing one, so the defect is multiplied by the page size; a caller that compares a summary from the list against the description of the same namespace still sees a self-consistent emulator, which is exactly why neither value can be caught locally and both must match the service.
  Fix: the RuleGroupsNamespaceSummary projection in ListRuleGroupsNamespaces' handler, which must build its status structure and its timestamps through the SAME shared projection the describe path uses once cause 1 is fixed, rather than repeating the mapping inline.
  Traces: amp-rule-groups-and-alert-manager-definition-round-trip#task#5

Cause 3 of 3: DescribeAlertManagerDefinition hard-codes status.statusReason to absent and shows the same over-precise timestamps.
  AWS: Smithy: DescribeAlertManagerDefinition (GET /workspaces/{workspaceId}/alertmanager/definition, 200, @readonly) returns required alertManagerDefinition (AlertManagerDefinitionDescription), whose required members are status (AlertManagerDefinitionStatus: required statusCode, optional statusReason, "If there is a failure, the reason for the failure"), data, createdAt and modifiedAt. Recorded behavior only: the recorded response carries status.statusReason present as the empty string, and createdAt/modifiedAt as epoch-seconds numbers of the asserted shape.
  Emulator: answers 200 with the right data and statusCode, but $.alertManagerDefinition.status.statusReason is absent — here not even read off the record, the handler passes a literal None for it — and $.alertManagerDefinition.createdAt / modifiedAt are full-precision clock floats.
  Why: the alert manager definition is a singleton per workspace that callers poll through this read to decide whether a put has settled and, on failure, why; a statusReason that is structurally absent on the success envelope means the member can never be relied on to exist, so polling code written against AWS reads a missing key instead of an empty reason.
  Fix: the AlertManagerDefinitionDescription projection in DescribeAlertManagerDefinition's handler — its status structure must carry the record's reason (and the service's empty reason when nothing failed) instead of a hard-coded None — plus the same timestamp-resolution fix as cause 1 for the definition record's stamped instants.
  Traces: amp-rule-groups-and-alert-manager-definition-round-trip#task#7

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 49 steps, 42 passed, 7 mismatched, score 85.7%; emulator_up=true, bootstrap_signal=false, steps_blocked=0, not_implemented=0). Seven divergence clusters were reported; four carry owned_by 20261029T223357Z-aps-emulator-authored-refusal-payloads-and-default-scraper.md and are skipped. The three clusters below carry empty stub_path AND empty owned_by, and are the three causes of this issue. Every cluster's affected_count is 1.

Cause 1 - fixture amp-rule-groups-and-alert-manager-definition-round-trip, step task#4, DescribeRuleGroupsNamespace [mismatch], status 200 expected and 200 received:
  $.ruleGroupsNamespace.status.statusReason: missing (expected '', got None)
  $.ruleGroupsNamespace.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228650.9825149)
  $.ruleGroupsNamespace.modifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228650.9825149)
  divergence_key: service=aps|DescribeRuleGroupsNamespace|status=200->200|keys=$.ruleGroupsNamespace.createdAt,$.ruleGroupsNamespace.modifiedAt,$.ruleGroupsNamespace.status.statusReason
  (The recorded instant is per-run and is NOT the requirement - the corpus asserts the timestamp's shape, not its value. The requirement being missed is the number shape: 1794228650.9825149 carries seven fractional digits.)
  Source: services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/describe_rule_groups_namespace.rs:204-211 - status_reason is copied from the record (which is None, services/.../src/ops/create_rule_groups_namespace.rs:347) and created_at/modified_at are DateTime::from(namespace.created_at) over the SystemTime::now() captured at create (same file, lines 189 and 348-349).

Cause 2 - fixture amp-rule-groups-and-alert-manager-definition-round-trip, step task#5, ListRuleGroupsNamespaces [mismatch], status 200 expected and 200 received:
  $.ruleGroupsNamespaces[0].status.statusReason: missing (expected '', got None)
  $.ruleGroupsNamespaces[0].createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228650.9825149)
  $.ruleGroupsNamespaces[0].modifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228650.9825149)
  divergence_key: service=aps|ListRuleGroupsNamespaces|status=200->200|keys=$.ruleGroupsNamespaces[0].createdAt,$.ruleGroupsNamespaces[0].modifiedAt,$.ruleGroupsNamespaces[0].status.statusReason
  Source: services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/list_rule_groups_namespaces.rs:248-257 - the summary projection repeats the same inline mapping as the describe path.

Cause 3 - fixture amp-rule-groups-and-alert-manager-definition-round-trip, step task#7, DescribeAlertManagerDefinition [mismatch], status 200 expected and 200 received:
  $.alertManagerDefinition.status.statusReason: missing (expected '', got None)
  $.alertManagerDefinition.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228656.0117517)
  $.alertManagerDefinition.modifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794228656.0117517)
  divergence_key: service=aps|DescribeAlertManagerDefinition|status=200->200|keys=$.alertManagerDefinition.createdAt,$.alertManagerDefinition.modifiedAt,$.alertManagerDefinition.status.statusReason
  Source: services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/describe_alert_manager_definition.rs:197-204 - status_reason is the literal None, and created_at/modified_at are DateTime::from of the stored SystemTime.

Model facts used (model_path services/service-com-amazonaws-amp-amazonprometheusservice/smithy-model/com-amazonaws-amp-amazonprometheusservice.json): RuleGroupsNamespaceStatus {statusCode required, statusReason optional String}; AlertManagerDefinitionStatus {statusCode required, statusReason optional String}; RuleGroupsNamespaceDescription / RuleGroupsNamespaceSummary / AlertManagerDefinitionDescription all require createdAt and modifiedAt targeting smithy.api#Timestamp (no timestampFormat override, so restJson1 epoch-seconds).

## Steps to Reproduce
Replay the whole corpus against the emulator to see all three at once (fixture amp-rule-groups-and-alert-manager-definition-round-trip, steps task#4, task#5 and task#7). Each cause also reproduces on its own:

Cause 1 (amp-rule-groups-and-alert-manager-definition-round-trip#task#4): create a workspace, wait until it is ACTIVE, POST /workspaces/{workspaceId}/rulegroupsnamespaces with a name and a base64 rules file, wait until the namespace is ACTIVE, then GET /workspaces/{workspaceId}/rulegroupsnamespaces/{name}. In the 200 body, $.ruleGroupsNamespace.status has no statusReason key where the service sends statusReason as the empty string, and $.ruleGroupsNamespace.createdAt / modifiedAt are epoch-seconds numbers with seven fractional digits instead of the service's at-most-millisecond resolution.

Cause 2 (amp-rule-groups-and-alert-manager-definition-round-trip#task#5): with that namespace in place, GET /workspaces/{workspaceId}/rulegroupsnamespaces. Each entry of $.ruleGroupsNamespaces shows the same two defects as cause 1.

Cause 3 (amp-rule-groups-and-alert-manager-definition-round-trip#task#7): PUT the alert manager definition for the workspace (POST /workspaces/{workspaceId}/alertmanager/definition), wait until it is ACTIVE, then GET /workspaces/{workspaceId}/alertmanager/definition. $.alertManagerDefinition.status has no statusReason key, and $.alertManagerDefinition.createdAt / modifiedAt again carry seven fractional digits.
