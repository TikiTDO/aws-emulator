Issue-Source: trace-sweep-divergence
Issue-Trace: config-resource-config-discovery-and-query
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-resource-config-discovery-and-query config-aggregate-query-surface config-configuration-aggregator-and-authorization-workflow config-list-evaluations-by-context-identifier config-proactive-evaluation-lifecycle config-proactive-resource-evaluation-lifecycle config-resource-evaluation-context-grouping config-resource-evaluation-grouped-by-context config-start-resource-evaluation-smoke config-minimal-evaluation-default-timeout config-stored-query-expression-unicode-round-trip config-stored-query-unicode-and-emoji-round-trip config-stored-query-unicode-expression-round-trip
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|ListDiscoveredResources|2xx->4xx|$.resourceIdentifiers
Issue-Key: config|ListDiscoveredResources|2xx->4xx|status
Issue-Key: config|BatchGetResourceConfig|2xx->4xx|$.baseConfigurationItems
Issue-Key: config|BatchGetResourceConfig|2xx->4xx|$.unprocessedResourceKeys
Issue-Key: config|BatchGetResourceConfig|2xx->4xx|status
Issue-Key: config|GetAggregateConformancePackComplianceSummary|2xx->2xx|$.AggregateConformancePackComplianceSummaries[*]
Issue-Key: config|DescribeConfigurationAggregatorSourcesStatus|2xx->2xx|$.AggregatedSourceStatusList[*]
Issue-Key: config|StartResourceEvaluation|2xx->2xx|$.ResourceEvaluationId
Issue-Key: config|GetResourceEvaluationSummary|2xx->2xx|$.EvaluationStatus.FailureReason
Issue-Key: config|GetStoredQuery|2xx->2xx|$.StoredQuery.QueryId
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# Config read paths over-refuse, fabricate aggregator rows and mint non-AWS ids

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoveredResources

## Description
Seven distinct causes on this service's read and id-minting paths: two discovery reads refuse a request AWS answers 200, two aggregator reads return the opposite of AWS's body, two generated identifiers are not AWS-shaped, and one evaluation failure reason is emulator prose. SEVEN CAUSES must be closed.

Cause 1 of 7 - ListDiscoveredResources refuses instead of returning an empty page (fixture config-resource-config-discovery-and-query, step task#2).
AWS: ListDiscoveredResources returns resourceIdentifiers (a list, no min) and models NoAvailableConfigurationRecorderException / ValidationException (Smithy: ListDiscoveredResources). The recording shows that in this account state AWS answers 200 with an empty resourceIdentifiers list - "nothing discovered" is a successful, empty result, not an error.
Emulator: answers 400 and omits resourceIdentifiers entirely.
Why: callers that enumerate discovered resources (and treat an empty page as "none yet") get a hard failure, so discovery-polling loops abort against the emulator where they succeed on AWS.
Fix: ListDiscoveredResources' handler - return 200 with an empty resourceIdentifiers list rather than raising the recorder-availability refusal on the read path.

Cause 2 of 7 - BatchGetResourceConfig refuses instead of returning empty lists (fixture config-resource-config-discovery-and-query, step task#3).
AWS: BatchGetResourceConfig output carries baseConfigurationItems and unprocessedResourceKeys (Smithy: BatchGetResourceConfig). The recording shows 200 with both lists present and empty when none of the requested keys resolves.
Emulator: answers 400 and omits both members.
Why: the documented AWS idiom - submit keys, read back what resolved and what did not - is unavailable; clients cannot distinguish "no configuration items" from a request error.
Fix: BatchGetResourceConfig's handler - answer 200 with empty baseConfigurationItems/unprocessedResourceKeys instead of refusing.

Cause 3 of 7 - GetAggregateConformancePackComplianceSummary returns an empty summary list (fixture config-aggregate-query-surface, step task#6).
AWS: the output carries AggregateConformancePackComplianceSummaries (Smithy: GetAggregateConformancePackComplianceSummary). The recording shows one summary entry whose ComplianceSummary reports zero counts (CompliantConformancePackCount 0, NonCompliantConformancePackCount 0) - AWS returns a zero-valued summary, not no summary.
Emulator: returns an empty AggregateConformancePackComplianceSummaries list, so index 0 is absent.
Why: a client reading the aggregate counts gets no row at all and cannot tell "aggregator has no non-compliant packs" from "aggregation not available"; dashboards that index the first summary fail.
Fix: GetAggregateConformancePackComplianceSummary's handler/state - emit a zero-valued summary row per group key instead of an empty list.

Cause 4 of 7 - DescribeConfigurationAggregatorSourcesStatus fabricates a SUCCEEDED source row (fixture config-configuration-aggregator-and-authorization-workflow, step task#5).
AWS: output carries AggregatedSourceStatusList (Smithy: DescribeConfigurationAggregatorSourcesStatus). The recording shows an empty AggregatedSourceStatusList at this point in the workflow - the aggregator's source status has not been published yet.
Emulator: immediately returns a row with SourceId <account>, SourceType ACCOUNT, AwsRegion, LastUpdateStatus 'SUCCEEDED' and LastUpdateTime.
Why: callers that poll until a source reports SUCCEEDED, or that assert the aggregator has not yet aggregated, see fabricated success; authorization/aggregation sequencing cannot be tested.
Fix: DescribeConfigurationAggregatorSourcesStatus' handler/state - report source status only once the aggregation source has actually produced one.

Cause 5 of 7 - StartResourceEvaluation mints a ResourceEvaluationId that is not AWS's id form (fixture config-list-evaluations-by-context-identifier, step task#1).
AWS: ResourceEvaluationId is a string with @length 1..128 (Smithy: shape ResourceEvaluationId); recorded behavior shows AWS returning an opaque, unordered, random-looking identifier - the recording asserts the id's SHAPE, not its literal value.
Emulator: returns a dense 16-character lowercase base-36 token whose leading characters are a monotonically increasing counter ('0002...'), so ids are sequential and predictable across calls and the shape assertion fails.
Why: clients and tests that validate the id's form, or that must not depend on ordering, behave differently; a predictable sequential id also lets callers guess other evaluations' ids, which AWS's form does not.
Fix: the id generator behind StartResourceEvaluation (shared with the evaluation state) - mint AWS-shaped opaque random ids.

Cause 6 of 7 - GetStoredQuery returns a QueryId that is not AWS's id form (fixture config-stored-query-expression-unicode-round-trip, step task#2).
AWS: StoredQuery.QueryId is @length 1..36 with pattern ^\S+$ (Smithy: shape StoredQuery) - the 36-character bound is AWS's uuid-form query id, and the recording asserts that shape rather than a literal value.
Emulator: returns the same 16-character sequential base-36 counter token as cause 5 ('0003...'), which is also embedded in the query ARN's /query-<id> segment.
Why: clients that validate or correlate the query id (and the ARN built from it) against AWS's uuid form fail; ids are again sequential and guessable.
Fix: the stored-query id generator and the QueryArn it feeds.

Cause 7 of 7 - GetResourceEvaluationSummary's FailureReason is emulator prose (fixture config-minimal-evaluation-default-timeout, step task#2).
AWS: EvaluationStatus carries a FailureReason (Smithy: GetResourceEvaluationSummary output -> EvaluationStatus). The recording shows the fixed, request-independent sentence "No Config rule is enabled in proactive mode for the given resource type."
Emulator: 'the PROACTIVE evaluation could not be run: this account has no Config rule that evaluates in PROACTIVE mode, so there was no rule configuration to evaluate the resource details against'.
Why: callers that surface or match on the AWS failure reason (the only machine-visible explanation for a failed proactive evaluation) see text AWS never sends.
Fix: the proactive-evaluation failure path that populates EvaluationStatus.FailureReason.

No existing open issue covers these read paths or the id generators.

Traces: config-resource-config-discovery-and-query#task#2 (ListDiscoveredResources) and #task#3 (BatchGetResourceConfig), config-aggregate-query-surface#task#6 (GetAggregateConformancePackComplianceSummary), config-configuration-aggregator-and-authorization-workflow#task#5 (DescribeConfigurationAggregatorSourcesStatus), config-list-evaluations-by-context-identifier#task#1 (StartResourceEvaluation, 9 steps in cluster), config-stored-query-expression-unicode-round-trip#task#2 (GetStoredQuery, 3 steps in cluster), config-minimal-evaluation-default-timeout#task#2 (GetResourceEvaluationSummary, 4 steps in cluster).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false).

Cause 1 - ListDiscoveredResources, config-resource-config-discovery-and-query#task#2 (2 steps in cluster):
  status: expected 200, got 400; $.resourceIdentifiers: missing (expected [], got None)

Cause 2 - BatchGetResourceConfig, config-resource-config-discovery-and-query#task#3:
  status: expected 200, got 400; $.baseConfigurationItems: missing (expected []); $.unprocessedResourceKeys: missing (expected [])

Cause 3 - GetAggregateConformancePackComplianceSummary, config-aggregate-query-surface#task#6 (200 both sides):
  $.AggregateConformancePackComplianceSummaries[0]: missing (expected {'ComplianceSummary': {'CompliantConformancePackCount': 0, 'NonCompliantConformancePackCount': 0}}, got None)

Cause 4 - DescribeConfigurationAggregatorSourcesStatus, config-configuration-aggregator-and-authorization-workflow#task#5 (200 both sides):
  $.AggregatedSourceStatusList[0]: extra (expected None, got {'SourceId': '000000000000', 'SourceType': 'ACCOUNT', 'AwsRegion': 'us-west-2', 'LastUpdateStatus': 'SUCCEEDED', 'LastUpdateTime': ...})

Cause 5 - StartResourceEvaluation, config-list-evaluations-by-context-identifier#task#1 (200 both sides, 9 steps in cluster):
  $.ResourceEvaluationId: shape assertion failed (expected AWS's opaque id form, got '0002wplmbeoct13p' - a 16-char sequential base-36 counter token)

Cause 6 - GetStoredQuery, config-stored-query-expression-unicode-round-trip#task#2 (200 both sides, 3 steps in cluster):
  $.StoredQuery.QueryId: shape assertion failed (expected AWS's uuid-form query id, got '0003fsnatkpgcyi7')

Cause 7 - GetResourceEvaluationSummary, config-minimal-evaluation-default-timeout#task#2 (200 both sides, 4 steps in cluster):
  $.EvaluationStatus.FailureReason expected 'No Config rule is enabled in proactive mode for the given resource type.'
  got 'the PROACTIVE evaluation could not be run: this account has no Config rule that evaluates in PROACTIVE mode, so there was no rule configuration to evaluate the resource details against'

(The literal id strings above are the emulator's per-run values and are quoted only to show their FORM; the account id, region and timestamps are per-run too.)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): ListDiscoveredResources output resourceIdentifiers; BatchGetResourceConfig output baseConfigurationItems + unprocessedResourceKeys; GetAggregateConformancePackComplianceSummary output AggregateConformancePackComplianceSummaries; DescribeConfigurationAggregatorSourcesStatus output AggregatedSourceStatusList; ResourceEvaluationId @length 1..128; StoredQuery.QueryId @length 1..36 pattern ^\S+$; GetResourceEvaluationSummary output EvaluationStatus.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: config-resource-config-discovery-and-query#task#2 - ListDiscoveredResources; 400 instead of 200 with an empty resourceIdentifiers list.
4. Cause 2: same fixture #task#3 - BatchGetResourceConfig; 400 instead of 200 with empty lists.
5. Cause 3: config-aggregate-query-surface#task#6 - GetAggregateConformancePackComplianceSummary; the zero-valued summary row is absent.
6. Cause 4: config-configuration-aggregator-and-authorization-workflow#task#5 - DescribeConfigurationAggregatorSourcesStatus; a SUCCEEDED ACCOUNT row is fabricated.
7. Cause 5: config-list-evaluations-by-context-identifier#task#1 - StartResourceEvaluation; inspect the ResourceEvaluationId form (sequential 16-char token).
8. Cause 6: config-stored-query-expression-unicode-round-trip#task#2 - PutStoredQuery then GetStoredQuery; inspect StoredQuery.QueryId form.
9. Cause 7: config-minimal-evaluation-default-timeout#task#2 - GetResourceEvaluationSummary for a proactive evaluation with no proactive rule; compare EvaluationStatus.FailureReason.
10. Re-replay all seven fixture#step bindings to confirm the fix.
