Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-association-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-association-lifecycle guardduty-association-list-filters-across-two-rules guardduty-custom-detection-rule-association-lifecycle
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/2790fbb31cd2c5b51b511ef613ef0002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/1c0dc8b40774691e2cbef1febc0f0002"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/4868cb5c9c8aa2ba35381b12e26d0003"].associationId
Issue-Key: guardduty|ListCustomDetectionRuleAssociations|2xx->2xx|$.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/49619d1de0e8cba73b531e35698f0002"].associationId
Issue-Commit: af62200c377d35a133e0c9542bd650848b4b14bd

# ListCustomDetectionRuleAssociations summaries report an associationId that does not round-trip the association identity

> _Note: this issue was identified against the code base at commit `af62200c377d35a133e0c9542bd650848b4b14bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCustomDetectionRuleAssociations

## Description
This issue bundles 4 distinct net-new replay clusters, ALL in the guardduty crate, ALL on ListCustomDetectionRuleAssociations, and ALL on the same member: the `associationId` an AssociationSummary advertises for an association that the fixture located by that same entry's `arn`. Fix ALL 4 and re-replay every fixture#step named under Traces before submitting. None of the 4 is marked owned by an open issue and none is blocked by a stub.

The shared fault: the identity a custom detection rule association is listed under is not interchangeable with the identity the caller established for it, so a list-then-act flow hands the client a handle the service does not accept. The deterministic comparator reports the member as a mismatch on all four steps even though the replay report RENDERS expected and actual with the same visible hex digits, which means the difference lies in a dimension the rendering collapses (JSON type, case, surrounding whitespace, or a duplicate entry selected by the ARN predicate) rather than in the digits themselves. Hypothesis: the list projection serializes an id that is byte-distinct from the id the create/get path bound for the same record, or the Region's store holds more than one record for one (region, ruleId) so the `[arn=...]` predicate resolves ambiguously; neither the model nor the recording states which, so the fixer must capture the raw list body and compare the serialized `associationId` member byte-for-byte against both the ARN's trailing `/association/<id>` segment and the id a following Get accepts as its path label.

Note for the fixer, so this is not closed as a duplicate: an OPEN issue, 20261107T064647Z-guardduty-custom-detection-rule-association-identity-is-not-stable.md, describes the same fault family and names these same four fixture#steps, but it keys its ownership on literal per-run association ids (its Issue-Key lines carry ARNs ending in ids such as e5d07a4c1109a6747205dd529f240002) and the replay no longer binds these clusters to it — the current run reports all four with `owned_by: []`, i.e. unowned and unfiled. A resolved issue, 20261103T231304Z-guardduty-custom-detection-rule-association-identity-diverges-across.md, is marked fixed and does NOT cover them either: the replay still reports them. Treat the list-summary identity path as still-open behaviour.

Cause 1 of 4: the association-lifecycle listing reports an `associationId` the comparator rejects for the entry the fixture selects by that entry's own ARN. Fixture guardduty-association-lifecycle, step task#7.
AWS: Smithy ListCustomDetectionRuleAssociations (GET /custom-detection-rule/association, success 200; input MaxResults @httpQuery maxResults @range 1..100, NextToken @httpQuery nextToken, RuleId @httpQuery ruleId @length 1..100 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, Mode @httpQuery mode (AssociationMode); @readonly; @paginated inputToken=NextToken outputToken=NextToken items=RuleAssociations pageSize=MaxResults; errors AccessDeniedException 403, BadRequestException 400, InternalServerErrorException 500) returns a required `ruleAssociations` list of AssociationSummary. Smithy AssociationSummary.AssociationId -> AssociationId, jsonName `associationId`, required, @length 1..64, @pattern ^[a-zA-Z0-9_-]{1,64}$, "The unique identifier for the association"; AssociationSummary.Arn -> DetectionRuleArn, jsonName `arn`, required, @length 1..1024, @pattern ^arn:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]+:[a-zA-Z0-9\-]*:([0-9]*|aws):.+$, "The Amazon Resource Name (ARN) of the association". Recorded behavior only for the VALUE: the recording pins no literal id, it pins the listed id to the association identity the lifecycle already established (the id create returned and Get accepted as its `AssociationId` @httpLabel), and requires one summary's `associationId` to be exactly the trailing `/association/<id>` segment of that same summary's `arn`. One association therefore has ONE id for the whole scenario and every projection naming it repeats that id byte for byte.
Emulator: HTTP 200 matches, but the replay reports `$.ruleAssociations[arn="arn:aws:guardduty:<region>:<account>:detection-rule/custom/<ruleId>/association/<id>"].associationId: value_mismatch`, with expected and actual rendering as the same characters. The id digits, the counter-suffixed shape of the minted id, and the region/account/ruleId parts of the ARN are per-run values and are NOT the requirement; the requirement is that the listed `associationId` is the association's established identity and is identical to its own ARN suffix.
Why: list-then-act is the normal discovery path. A client that lists associations and then calls Get/Update/Delete with the id a summary advertises operates on a handle the store does not resolve (Get is keyed on (RuleId, AssociationId) @httpLabel), so the follow-up 404s; ARN-keyed IAM policy and inventory joins written against the listed ARN also stop matching.
Fix: the AssociationSummary projection of the list handler and the shared association view/ARN helper it is derived from, so create, get and list serialize ONE identity for one record.
Traces: guardduty-association-lifecycle#task#7 (affected_count 1).

Cause 2 of 4: the same list-summary identity divergence on the FIRST rule of the two-rule filtered listing. Fixture guardduty-association-list-filters-across-two-rules, step task#5.
AWS: same ListCustomDetectionRuleAssociations / AssociationSummary citation as cause 1, with the RuleId @httpQuery filter narrowing by equality on the summary's `ruleId`. Recorded behavior only: a filtered listing reports each association under the identity it was created with, and each entry's ARN suffix equals that entry's `associationId`; filtering changes WHICH entries appear, never the identity an entry is reported under.
Emulator: 200 matches; the entry located by its own `arn` (ruleId `admin-policy-attached-to-role`) reports a mismatching `associationId`, rendered with the same digits as expected.
Why: a caller that narrows discovery by RuleId cannot trust the ids it reads back, so a filtered discovery pass yields handles that do not resolve on the next call.
Fix: same list-handler projection as cause 1; listed separately because it is its own replay cluster with its own fixture and step and must be re-replayed on its own.
Traces: guardduty-association-list-filters-across-two-rules#task#5 (affected_count 1).

Cause 3 of 4: the same divergence on the SECOND rule of that filtered listing — a different ruleId, hence a distinct association record and a distinct cluster. Fixture guardduty-association-list-filters-across-two-rules, step task#6.
AWS: same citation as cause 1. Recorded behavior only: the entry for the second rule reports its own association's identity, and its ARN suffix equals its own `associationId`.
Emulator: 200 matches; the entry located by its `arn` (ruleId `admin-policy-attached-to-user`) reports a mismatching `associationId`, again rendered identically to expected.
Why: the defect is per-association rather than per-listing, so a caller listing across two rules gets unusable handles for both and neither rule's association can be acted on from the listing.
Fix: same list-handler projection as cause 1.
Traces: guardduty-association-list-filters-across-two-rules#task#6 (affected_count 1).

Cause 4 of 4: the same divergence on the second association-lifecycle fixture, after create/get/update. Fixture guardduty-custom-detection-rule-association-lifecycle, step task#7.
AWS: same citation as cause 1. Recorded behavior only: after the lifecycle's create, read-back and update, the listing reports the association under the identity the lifecycle established, with the ARN's trailing segment equal to that entry's `associationId`.
Emulator: 200 matches; the entry selected by its `arn` reports a mismatching `associationId`, rendered identically to expected.
Why: the lifecycle's final discovery step hands the caller an id that does not round-trip, so a delete issued from the listing addresses nothing and the association is leaked.
Fix: same list-handler projection as cause 1.
Traces: guardduty-custom-detection-rule-association-lifecycle#task#7 (affected_count 1).

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the booted emulator: trace_set_id=simparity-wire-aws-v1, emulator_up=true (/__admin/health answered 200 and the :35443 data listener accepts), bootstrap_signal=false, fixtures_replayed=86, steps_total=559, steps_passed=524, steps_mismatched=35, steps_errored=0, steps_blocked=0, not_implemented=0, score_pct=93.7. 22 divergence clusters; 18 are marked owned by open guardduty issues, and the 4 below carry no `owned_by` and no `stub_path`. Expected = the recorded AWS response, got = the emulator.

Cause 1 — guardduty-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected status 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/2790fbb31cd2c5b51b511ef613ef0002"].associationId: value_mismatch (expected '2790fbb31cd2c5b51b511ef613ef0002', got '2790fbb31cd2c5b51b511ef613ef0002')
  divergence_key: service=guardduty|ListCustomDetectionRuleAssociations|status=200->200|keys=$.ruleAssociations[arn="...association/2790fbb31cd2c5b51b511ef613ef0002"].associationId
  affected_count 1; owned_by []; stub_path "".
  The two rendered values print identically, so the compared difference is not in the visible digits; the deterministic comparator still reports the member as a mismatch. The id digits (and their '0002' counter suffix) are per-run values, not requirements.

Cause 2 — guardduty-association-list-filters-across-two-rules#task#5, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/1c0dc8b40774691e2cbef1febc0f0002"].associationId: value_mismatch (expected '1c0dc8b40774691e2cbef1febc0f0002', got '1c0dc8b40774691e2cbef1febc0f0002')
  affected_count 1; owned_by []; stub_path "".

Cause 3 — guardduty-association-list-filters-across-two-rules#task#6, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-user/association/4868cb5c9c8aa2ba35381b12e26d0003"].associationId: value_mismatch (expected '4868cb5c9c8aa2ba35381b12e26d0003', got '4868cb5c9c8aa2ba35381b12e26d0003')
  affected_count 1; owned_by []; stub_path "". Note the different ruleId (admin-policy-attached-to-user), i.e. a second association record.

Cause 4 — guardduty-custom-detection-rule-association-lifecycle#task#7, ListCustomDetectionRuleAssociations, expected 200, got 200:
  $.ruleAssociations[arn="arn:aws:guardduty:us-west-2:000000000000:detection-rule/custom/admin-policy-attached-to-role/association/49619d1de0e8cba73b531e35698f0002"].associationId: value_mismatch (expected '49619d1de0e8cba73b531e35698f0002', got '49619d1de0e8cba73b531e35698f0002')
  affected_count 1; owned_by []; stub_path "".

Related clusters in the same run that are NOT part of this issue because the replay marks them owned: the CreateCustomDetectionRuleAssociation clusters at guardduty-association-duplicate-conflict#task#3 (expected '{{assoc_conflict_id}}', got '883dfc561a2f6d6d37a278d5fed90002') and guardduty-association-lifecycle#task#3 (expected '{{assoc_lifecycle_id}}', got '2790fbb31cd2c5b51b511ef613ef0002'), and the GetCustomDetectionRuleAssociation cluster at guardduty-association-duplicate-conflict#task#5 — all owned by 20261107T064647Z-guardduty-custom-detection-rule-association-identity-is-not-stable.md. They are cited only to show that create re-mints the identity the list then reports, which is why the list divergence is an identity defect and not a formatting one.

Supporting code reading (for the fix, not a requirement): services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/list_custom_detection_rule_associations.rs lines 248-264 (`summary`) builds each AssociationSummary with `association_id: record.association_id.clone()` and derives `arn` through `crate::state::custom_detection_rule_association_arn(&record.region, &record.rule_id, &record.association_id)` — so within one payload the two are built from the same field, which is why the renderer shows identical digits and why the surviving difference must be found by comparing the serialized bytes of the member against the ARN suffix and against the id a following Get accepts. Lines 179-190 select by `record.region == region` over the `(region, RuleId)`-keyed `custom_detection_rule_associations` map; if that map can hold more than one record whose ARN renders the same, the `[arn=...]` predicate resolves ambiguously, which is the other candidate root cause.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the simparity-wire-aws-v1 wire corpus. Confirm emulator_up=true and bootstrap_signal=false before reading any divergence.
2. Cause 1: replay fixture guardduty-association-lifecycle through step task#7 (GET /custom-detection-rule/association). Capture the RAW response body. For the entry whose `arn` ends '/association/<id>', compare that entry's serialized `associationId` member byte-for-byte with the ARN's trailing segment and with the id the fixture's earlier create returned and its Get used as the AssociationId path label — including JSON type, case and whitespace, since the two values render identically in the replay report. Also assert the page contains exactly ONE entry per (ruleId) for the Region.
3. Cause 2: replay fixture guardduty-association-list-filters-across-two-rules, step task#5 (listing filtered to ruleId 'admin-policy-attached-to-role') and make the same byte-for-byte comparison for that entry.
4. Cause 3: same fixture, step task#6 (ruleId 'admin-policy-attached-to-user') and repeat the comparison for that second association record.
5. Cause 4: replay fixture guardduty-custom-detection-rule-association-lifecycle, step task#7, and repeat the comparison after the lifecycle's create/get/update.
6. Harness-free repro: ensure a detector exists (ListDetectors, else CreateDetector(Enable=true)); ListCustomDetectionRules to pick a published ruleId (e.g. 'admin-policy-attached-to-role'); CreateCustomDetectionRuleAssociation(RuleId=<ruleId>, Mode='DRY_RUN') and record the returned associationId A1; GetCustomDetectionRuleAssociation(RuleId=<ruleId>, AssociationId=A1) and confirm it resolves; then ListCustomDetectionRuleAssociations() with no filter, with RuleId=<ruleId>, and with Mode='DRY_RUN'. For every summary returned, assert (a) `associationId` is byte-identical to the trailing '/association/<id>' segment of that summary's own `arn`, (b) `associationId` is byte-identical to A1, and (c) GetCustomDetectionRuleAssociation(RuleId=<summary.ruleId>, AssociationId=<summary.associationId>) succeeds with 200. Repeat with a second ruleId so two records are listed together.
7. Re-replay all four fixture#steps (guardduty-association-lifecycle#task#7, guardduty-association-list-filters-across-two-rules#task#5, guardduty-association-list-filters-across-two-rules#task#6, guardduty-custom-detection-rule-association-lifecycle#task#7) and confirm all four clusters pass before submitting.
