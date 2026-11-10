Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.conformance-pack-compliance-details-and-summary-consistent config.conformance-pack-deploy-compliance-summary config.aggregate-conformance-pack-compliance-summary-reflects-pack-status config.aggregate-config-rule-compliance-summary-matches-details config.batch-get-aggregate-resource-config-matches-individual-gets config.put-conformance-pack-invalid-template-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.conformance-pack-compliance-details-and-summary-consistent|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: config.conformance-pack-deploy-compliance-summary|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: config.aggregate-conformance-pack-compliance-summary-reflects-pack-status|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: config.aggregate-config-rule-compliance-summary-matches-details|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: config.batch-get-aggregate-resource-config-matches-individual-gets|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: config.put-conformance-pack-invalid-template-rejected|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# config: compliance and aggregation never converge — conformance-pack rule results, a CUSTOM_LAMBDA verdict, aggregator discovery, and a pack delete that never lands (4 causes)

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConformancePackComplianceDetails

## Description
This issue bundles 4 DISTINCT net-new root causes in the config service, each grounded in failing behavioral checks from this replay (every one of them hung to the harness's 60 s cap because the state it polls for never arrives). They are numbered Cause 1 of 4 .. Cause 4 of 4 and each carries its own failing check ids; a fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: a conformance pack's rules are never evaluated, so the pack's compliance surface never produces a result ===
AWS: Smithy: GetConformancePackComplianceDetails returns ConformancePackRuleEvaluationResults and GetConformancePackComplianceSummary returns ConformancePackComplianceSummaryList (len 1..5) whose ConformancePackComplianceStatus is COMPLIANT or NON_COMPLIANT. Recorded behavior: a pack deployed from a template declaring one AWS Config MANAGED rule (Owner: AWS, SourceIdentifier: IAM_PASSWORD_POLICY) reaches CREATE_COMPLETE and then its rule is EVALUATED, so the details read returns at least one ConformancePackEvaluationResult and the pack-level summary settles on COMPLIANT/NON_COMPLIANT consistent with those rows (NON_COMPLIANT if any row is); the self-aggregator's GetAggregateConformancePackComplianceSummary then reports exactly one pack on the matching side.
Emulator: the pack reaches CREATE_COMPLETE (deploy settles after ~500 ms) and its rules ARE created, but nothing ever evaluates them — PutConformancePack's own handler documentation states it: "What the deployment does NOT do is evaluate those rules against resources: an evaluation result is recorded by the evaluation writers, which are scaffolded, so the pack's rules report the INSUFFICIENT_DATA of a rule nothing has evaluated". GetConformancePackComplianceDetails therefore returns an empty ConformancePackRuleEvaluationResults forever, the pack summary never leaves its never-evaluated state, and the aggregate pack summary stays at compliant+noncompliant == 0.
Why: the whole conformance-pack compliance surface is unusable — a client waiting for a deployed pack's verdict (console, drift report, CI gate) polls forever and can never distinguish "still evaluating" from "will never evaluate".
Fix: the evaluation writer behind a deployed pack's managed rules (the state that records EvaluationResultEntry rows for a rule), which GetConformancePackComplianceDetails / GetConformancePackComplianceSummary / GetAggregateConformancePackComplianceSummary then report from.
Traces: config.conformance-pack-compliance-details-and-summary-consistent, config.conformance-pack-deploy-compliance-summary, config.aggregate-conformance-pack-compliance-summary-reflects-pack-status (all "Check timed out after 60.0s"); the gap is stated in services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_conformance_pack.rs (module doc, "What a deployment here does") and read in src/ops/get_conformance_pack_compliance_details.rs.

=== Cause 2 of 4: a scheduled CUSTOM_LAMBDA rule never delivers an evaluation, so the aggregate rule-compliance summary never moves ===
AWS: Recorded behavior only (the model carries no behavior here): a brand-new Lambda-backed Config rule is auto-evaluated on creation and can be re-triggered with StartConfigRulesEvaluation; the evaluator's PutEvaluations verdict then appears in GetComplianceDetailsByConfigRule for the rule, the self-aggregator's GetAggregateConfigRuleComplianceSummary non-compliant-rule count grows by exactly one, and GetAggregateComplianceDetailsByConfigRule reports exactly one NON_COMPLIANT row keyed to the evaluated resource id.
Emulator: StartConfigRulesEvaluation is accepted but the rule's Lambda evaluator is never invoked and no evaluation result is ever recorded, so GetComplianceDetailsByConfigRule stays empty and the aggregate summary/detail reads never move.
Why: no custom-rule compliance can ever be observed, locally or through an aggregator; a client that creates a rule and waits for its first verdict waits forever.
Fix: the rule-evaluation driver that invokes a CUSTOM_LAMBDA rule's evaluator (and the PutEvaluations write path it feeds), plus the aggregate rule-compliance read that rolls it up.
Traces: config.aggregate-config-rule-compliance-summary-matches-details ("Check timed out after 60.0s"; it hangs on the LOCAL GetComplianceDetailsByConfigRule poll, before any aggregate read). The open issue 20261029T065729Z-config-2-net-new-behavioral-root-causes-a.md describes the same mechanism for 5 OTHER checks and does not carry this check's replay key — this check must be re-replayed to confirm the fix.

=== Cause 3 of 4: a configuration aggregator never discovers the source account's resources ===
AWS: Smithy: GetAggregateResourceConfig models ResourceNotDiscoveredException; BatchGetAggregateResourceConfig returns BaseConfigurationItems plus UnprocessedResourceIdentifiers. Recorded behavior: with a running recorder and an aggregator whose AccountAggregationSources name this account and region, resources the recorder discovers (here two IAM roles, keyed by their immutable RoleId) become readable through GetAggregateResourceConfig, and BatchGetAggregateResourceConfig then returns one BaseConfigurationItem per identifier with no unprocessed identifiers, whose resourceId/resourceType/arn/accountId match the individual reads and whose resourceName is the role name.
Emulator: GetAggregateResourceConfig keeps answering ResourceNotDiscoveredException for both roles for the entire poll window, so the aggregate read path never has an item to compare; the aggregator aggregates nothing from its own account/region source.
Why: every aggregate data-plane read is permanently empty, so a multi-account/region inventory built on an aggregator reports no resources at all, with a not-found error rather than an empty result.
Fix: the aggregation pass that populates an aggregator's discovered-resource view from the account's recorded configuration items (state), which the aggregate single/batch reads serve.
Traces: config.batch-get-aggregate-resource-config-matches-individual-gets ("Check timed out after 60.0s" while polling for both roles to be discovered). Related but distinct from the open recorder issues 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md and 20261025T143826Z-config-iam-a-configuration-recorder-with-includeglobalresourcetypes-records.md, which are about the recorder's own history and do not carry this check's key.

=== Cause 4 of 4: DeleteConformancePack answers 501 NotImplemented, so an accepted pack can never be removed ===
AWS: Smithy: DeleteConformancePack takes ConformancePackName and models NoSuchConformancePackException and ResourceInUseException; after a successful delete an independent DescribeConformancePacks for that name is refused NoSuchConformancePackException with message "No such conformance pack exists. Conformance Pack(s): [<name>]" (fixed text plus the requested name).
Emulator: the handler is an unimplemented scaffold stub — it returns Scaffolded::not_implemented("DeleteConformancePack") (HTTP 501) — so the pack is never removed and DescribeConformancePacks keeps reporting it; a client that swallows the delete error and then polls for the pack to disappear never terminates, which is how this check reaches the 60 s cap rather than an assertion. The invalid-template refusal the same check asserts (400 ConformancePackTemplateValidationException, "Template passed in the input parameter is invalid") is already correct.
Why: a conformance pack's lifecycle cannot be completed; packs accumulate and the quota gate in PutConformancePack eventually refuses new ones.
Fix: implement the DeleteConformancePack handler (remove the pack and the resources its deployment created, refuse an in-flight workflow with ResourceInUseException). The operation itself is already filed as 20261026T102206Z-config-deleteconformancepack-stub.md (and re-filed by this run's ALREADY-FILED entry for config.conformance-pack-lifecycle); this cause exists to carry the SECOND check that stub blocks, whose replay key no filed issue holds.
Traces: config.put-conformance-pack-invalid-template-rejected ("Check timed out after 60.0s" on the post-delete "proves gone" poll); handler services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/delete_conformance_pack.rs line 189.

## Evidence
Replay output, config service — all six checks fail as "Check timed out after 60.0s" because the polled state never arrives:

Cause 1: config.conformance-pack-compliance-details-and-summary-consistent, config.conformance-pack-deploy-compliance-summary, config.aggregate-conformance-pack-compliance-summary-reflects-pack-status. Expected: GetConformancePackComplianceDetails returns >0 ConformancePackRuleEvaluationResults for a CREATE_COMPLETE pack holding one managed rule, GetConformancePackComplianceSummary reports COMPLIANT/NON_COMPLIANT, and GetAggregateConformancePackComplianceSummary reports compliant+noncompliant == 1. Actual: details stay empty and both summaries never reach a compliance status. Corroborated in the emulator's own source: put_conformance_pack.rs module doc — "What the deployment does NOT do is evaluate those rules against resources ... so the pack's rules report the INSUFFICIENT_DATA of a rule nothing has evaluated".

Cause 2: config.aggregate-config-rule-compliance-summary-matches-details. Expected: after StartConfigRulesEvaluation on a scheduled CUSTOM_LAMBDA rule, GetComplianceDetailsByConfigRule returns a NON_COMPLIANT EvaluationResult and the aggregator's non-compliant rule count grows by exactly 1. Actual: the local details read returns [] for the whole window (the check never reaches its aggregate assertions).

Cause 3: config.batch-get-aggregate-resource-config-matches-individual-gets. Expected: GetAggregateResourceConfig resolves both created IAM roles (by RoleId) so BatchGetAggregateResourceConfig returns 2 BaseConfigurationItems with UnprocessedResourceIdentifiers == []. Actual: both identifiers keep raising ResourceNotDiscoveredException; 0/2 discovered.

Cause 4: config.put-conformance-pack-invalid-template-rejected. Expected: DeleteConformancePack removes the accepted positive-control pack and DescribeConformancePacks then raises NoSuchConformancePackException "No such conformance pack exists. Conformance Pack(s): [<pack>]". Actual: DeleteConformancePack answers 501 NotImplemented (scaffold stub at delete_conformance_pack.rs:189), the pack stays listed, and the "proves gone" poll never terminates.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite; the six checks in trace_ids all time out. Manually:

Cause 1: put-configuration-recorder + put-delivery-channel + start-configuration-recorder; put-conformance-pack with a template declaring one AWS::Config::ConfigRule (Owner: AWS, SourceIdentifier: IAM_PASSWORD_POLICY); wait for describe-conformance-pack-status == CREATE_COMPLETE; then poll get-conformance-pack-compliance-details (stays empty) and get-conformance-pack-compliance-summary (never COMPLIANT/NON_COMPLIANT); with a self-aggregator, get-aggregate-conformance-pack-compliance-summary stays all-zero.

Cause 2: create a Lambda-backed (CUSTOM_LAMBDA) scheduled rule, call start-config-rules-evaluation, then poll get-compliance-details-by-config-rule — EvaluationResults stays [].

Cause 3: with a running recorder, create two IAM roles, put-configuration-aggregator for this account+region, then poll get-aggregate-resource-config with {SourceAccountId, SourceRegion, ResourceId: <RoleId>, ResourceType: AWS::IAM::Role} — ResourceNotDiscoveredException forever; batch-get-aggregate-resource-config returns both identifiers unprocessed.

Cause 4: put-conformance-pack with a valid template, wait for CREATE_COMPLETE, call delete-conformance-pack (501 NotImplemented), then describe-conformance-packs --conformance-pack-names <pack> — it is still reported instead of NoSuchConformancePackException.
