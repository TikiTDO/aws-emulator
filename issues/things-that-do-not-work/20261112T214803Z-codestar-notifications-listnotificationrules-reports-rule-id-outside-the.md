Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-list-and-filter-rules
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-list-and-filter-rules
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/782ed9bb-b2ad-41bf-85c8-bca4dc0869ac"].Id
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/a0b47622-96ad-46c2-8355-9f049380ce74"].Id
Issue-Commit: 4a9f818e4a3e64a932f982b054384e5559103763

# codestar-notifications ListNotificationRules reports rule Id outside the recorded AWS rule-id form

> _Note: this issue was identified against the code base at commit `4a9f818e4a3e64a932f982b054384e5559103763`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationRules

## Description
Three replay clusters, all service=codestar-notifications, operation=ListNotificationRules, all answering the modelled HTTP 200 with the modelled members, diverge on ONE value: the `Id` the emulator reports for each listed notification rule. A fixer must close all THREE causes below before submitting.

Shared root cause. The rule id is minted by `crate::state::new_rule_id` (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:569): it takes a sha-256 digest of the string "notificationrule/<sequence>/<wall-clock nanos>" and lays the hex digest out as a hyphenated 36-character version-4-shaped UUID ("{8hex}-{4hex}-4{3hex}-8{3hex}-{12hex}"). `crate::state::notification_rule_arn` embeds that same value as the ARN's `notificationrule/<id>` segment, and `ListNotificationRules` echoes the stored value straight into `NotificationRuleSummary.Id` (src/ops/list_notification_rules.rs:193-201, `id: Some(rule.id.clone())`). So one minting decision in state decides the id every listing reports, on every page, filtered or not. Because the emulator keeps `Id` byte-identical to its own ARN segment, the replay detail prints an expected and an actual that LOOK the same (the assertion resolves the entry by its `Arn`, and that ARN carries the emulator-minted id): the diverging thing is the FORM of the minted identifier against the id form the recording holds, which is why the verdict is `mismatch` identically on every replay even though the concrete value is per-run.

Cause 1 of 3: unfiltered ListNotificationRules page — every entry's `Id` diverges (two of two listed rules). Fixture codestar-notifications-list-and-filter-rules, step task#6.
  AWS: Smithy: ListNotificationRules (POST /listNotificationRules, HTTP 200, @paginated inputToken=NextToken outputToken=NextToken items=NotificationRules pageSize=MaxResults) -> ListNotificationRulesResult.NotificationRules targets NotificationRuleBatch of NotificationRuleSummary{Id -> NotificationRuleId @length min 1 max 40, doc "The unique ID of the notification rule"; Arn -> NotificationRuleArn @pattern ^arn:aws[^:\\s]*:codestar-notifications:[^:\\s]+:\\d{12}:notificationrule\\/(.*\\S)?$}. The model constrains only the id's LENGTH — no pattern — so for the id's internal form this is recorded behavior only: the recording shows AWS reporting, for each rule, an `Id` equal to the id segment of that rule's ARN, in the id form AWS itself mints.
  Emulator: answers 200 with the right rules and the right members, and the replay reports `value_mismatch` at $.NotificationRules[Arn="arn:aws:codestar-notifications:<region>:<account>:notificationrule/<id>"].Id for BOTH entries of the page, because the id carried there was minted by `new_rule_id` rather than in the recorded form.
  Why: `Id` is the member a client uses to correlate a listing entry with a rule it already knows (and the only identifier the summary carries besides the ARN). A listing whose ids are not in the form AWS issues cannot be matched against ids a client recorded, persisted or logged from AWS, for every rule in the account.
  Fix: the id minting in `crate::state::new_rule_id` (state.rs:569) and, through it, `crate::state::notification_rule_arn`; NOT the ListNotificationRules handler, which already reports the stored value verbatim and must keep `Id` byte-for-byte identical to its ARN's resource segment (src/ops/list_notification_rules.rs:193-201). Nothing in the pagination path is implicated.
  Traces: codestar-notifications-list-and-filter-rules#task#6.

Cause 2 of 3: the same `Id` divergence on a FILTERED page returning one rule. Fixture codestar-notifications-list-and-filter-rules, step task#7.
  AWS: as cause 1 — Smithy: NotificationRuleSummary.Id -> NotificationRuleId @length 1..40 (form: recorded behavior only); ListNotificationRules input Filters -> ListNotificationRulesFilters. Filtering narrows WHICH rules are listed and changes nothing about the identity reported for a listed rule.
  Emulator: 200, the filter selects the correct rule (its ARN is matched by the assertion), and the single entry's $.NotificationRules[Arn="..."].Id still diverges.
  Why: a filtered listing is the normal way a client finds one rule by resource, event type, target or creator; the one entry it gets back carries an identifier it cannot match against the one AWS issued.
  Fix: same component as cause 1 (`crate::state::new_rule_id`). The filter predicate `rule_satisfies` (src/ops/list_notification_rules.rs) is NOT implicated — selection is correct.
  Traces: codestar-notifications-list-and-filter-rules#task#7.

Cause 3 of 3: the same `Id` divergence on the other filtered page. Fixture codestar-notifications-list-and-filter-rules, step task#8.
  AWS: as cause 1/2.
  Emulator: 200, correct rule selected, $.NotificationRules[Arn="..."].Id diverges for the single returned entry.
  Why: as cause 2 — every listing this service serves, filtered or unfiltered, reports a rule id a client cannot reconcile with AWS's.
  Fix: as cause 1/2.
  Traces: codestar-notifications-list-and-filter-rules#task#8.

No status, error code or error message is involved in any of the three causes: each step returned the modelled 200 with the modelled members, so the divergence is purely in the reported identity value.

Related open issue, which does NOT cover these clusters: 20261110T174500Z-codestar-notifications-rule-identifiers-diverge-createnotificationrule-arn-shape.md argues the same root cause (rule-id minting) and lists these three steps among its causes, but its replay keys (Issue-Key lines) embed the per-run ARNs of an EARLIER replay, so this run's three ListNotificationRules clusters do not map to it — the replay report returns owned_by=[] for all three, i.e. they carry no owning replay key and a fixer re-replaying that issue's keys would not exercise them. The CreateNotificationRule `$.Arn` cluster of this same run IS owned by that issue and is deliberately excluded here. The other open codestar-notifications issues (20261102T083845Z on CreateNotificationRule EventTypeIds validation, 20261029T183307Z on the ListTagsForResource stub) touch neither rule-id minting nor listings.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator: emulator_up=true, bootstrap_signal=false, 11 fixtures replayed, 95 steps total, 87 passed, 8 mismatched in 4 clusters, score 91.6%. All 4 clusters are service=codestar-notifications, verdict=mismatch, expected_status=200 / actual_status=200, stub_path empty. Three of them (the ones filed here) have owned_by=[]; the fourth (CreateNotificationRule $.Arn, codestar-notifications-rule-lifecycle#task#4) is owned by 20261110T174500Z-... and is excluded.

Cause 1 — fixture codestar-notifications-list-and-filter-rules, step task#6, operation ListNotificationRules, affected=["codestar-notifications-list-and-filter-rules#task#6"], divergence_key service=codestar-notifications|ListNotificationRules|status=200->200|keys=$.NotificationRules[Arn="...notificationrule/782ed9bb-b2ad-41bf-85c8-bca4dc0869ac"].Id,$.NotificationRules[Arn="...notificationrule/a0b47622-96ad-46c2-8355-9f049380ce74"].Id
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/782ed9bb-b2ad-41bf-85c8-bca4dc0869ac"].Id: value_mismatch; and $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/a0b47622-96ad-46c2-8355-9f049380ce74"].Id: value_mismatch — two of two listed rules.
  Expected: each summary's `Id` equal to the recorded rule id for that rule — the id segment of that rule's ARN in the form AWS mints (NotificationRuleId, @length 1..40, no modelled pattern).
  Actual: the emulator's own sha256-derived, UUID-shaped 36-character id from state::new_rule_id, reported for every entry of the page. The literal id strings are per-run (this run 782ed9bb-b2ad-41bf-85c8-bca4dc0869ac and a0b47622-96ad-46c2-8355-9f049380ce74; an earlier replay of the same steps produced 2dcfbdba-7c89-4c63-80dc-9fcea8f86cc3 and 7b85c7a4-d193-47e3-8263-5f8226b949f1) while the mismatch verdict is identical on every replay, so the defect is the FORM of the minted id, not any one recorded value. Because the emulator keeps `Id` identical to the id segment of its own ARN, the detail's printed expected and actual coincide — the entry is resolved by that same emulator-minted ARN.

Cause 2 — same fixture, step task#7, affected=["codestar-notifications-list-and-filter-rules#task#7"]:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/a0b47622-96ad-46c2-8355-9f049380ce74"].Id: value_mismatch — the single entry of a filtered page. Expected: the recorded rule id for the selected rule. Actual: the emulator-minted UUID-shaped id.

Cause 3 — same fixture, step task#8, affected=["codestar-notifications-list-and-filter-rules#task#8"]:
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/782ed9bb-b2ad-41bf-85c8-bca4dc0869ac"].Id: value_mismatch — the single entry of the other filtered page. Expected/actual as cause 2.

Code under all three causes (same checkout the emulator is built from):
  services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:569-586 `new_rule_id(sequence)` — sha256("notificationrule/<sequence>/<nanos>") laid out as "{8}-{4}-4{3}-8{3}-{12}"; its own doc comment calls this "the shape AWS rule ids take".
  .../src/state.rs (notification_rule_arn) — embeds that id as the ARN's "notificationrule/<id>" segment.
  .../src/ops/list_notification_rules.rs:193-201 — NotificationRuleSummary { id: Some(rule.id.clone()), arn: Some(rule.arn.clone()) }, for both the unfiltered and the filtered path.

Smithy model consulted (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): ListNotificationRules POST /listNotificationRules code 200, errors InvalidNextTokenException(400)/ValidationException(400), @paginated items=NotificationRules; NotificationRuleSummary.Id -> NotificationRuleId (string, @length min 1 max 40, NO pattern); NotificationRuleSummary.Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) against the booted emulator.
3. Cause 1: replay fixture codestar-notifications-list-and-filter-rules through step task#6 — an unfiltered ListNotificationRules (POST /listNotificationRules) after the fixture's rules have been created. The response is 200 with both rules listed, and the step diverges at $.NotificationRules[Arn="..."].Id for BOTH entries.
4. Cause 2: continue to step task#7 — a filtered ListNotificationRules returning one rule. 200, correct rule selected, $.NotificationRules[Arn="..."].Id diverges.
5. Cause 3: continue to step task#8 — the other filtered ListNotificationRules. 200, correct rule selected, $.NotificationRules[Arn="..."].Id diverges.
6. Confirm the form, not one value, is at fault: replay the corpus twice. The reported ids change between runs (state::new_rule_id mixes the wall clock into the digest) while all three clusters diverge identically every time.
7. After a fix, re-replay codestar-notifications-list-and-filter-rules#task#6, #task#7 and #task#8; all three must pass, and each listing's `Id` must stay byte-for-byte identical to the id segment of that rule's ARN and within NotificationRuleId's @length 1..40.
