Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-notifications-list-and-filter-rules
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-notifications-list-and-filter-rules
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ab795b6f-bef3-440d-8f04-49a65eac67f0"].Id
Issue-Key: codestar-notifications|ListNotificationRules|2xx->2xx|$.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4"].Id
Issue-Commit: 7753983c6e495ec3612695b1336bf4936fc7b7fd

# codestar-notifications ListNotificationRules reports unmatchable NotificationRuleSummary Id on every page

> _Note: this issue was identified against the code base at commit `7753983c6e495ec3612695b1336bf4936fc7b7fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationRules

## Description
Three replay clusters, all service=codestar-notifications / operation=ListNotificationRules / expected_status=200 -> actual_status=200, diverge on the SAME member: NotificationRuleSummary.Id. Every step answers the modeled 200 with the modeled members and selects the right rules; what diverges is the rule identity the listing reports, for EVERY entry of EVERY page, filtered or not. A fixer must close all THREE causes below before submitting.

Shared root cause. The id a listing reports is the id state minted for the rule: crate::state::new_rule_id takes a sha-256 digest of "notificationrule/<sequence>/<wall-clock nanos>" and lays it out as a hyphenated 36-character version-4-shaped UUID (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/state.rs:562-586), crate::state::notification_rule_arn embeds that same value as the ARN's notificationrule/<id> segment (state.rs:366-374), and ListNotificationRules echoes the stored id into NotificationRuleSummary.Id (src/ops/list_notification_rules.rs:193-201). So one minting decision decides what every listing reports. The replay detail prints the expected and the actual id with the same text, so the divergence is NOT a visible textual difference in one recording's value but the id's FORM/provenance: the comparator binds a listed entry by its Arn and still rejects the Id the emulator carries for it, on every replay, while the printed value changes from run to run. Hypothesis: the recorded id for a rule is not reconstructible from the emulator's per-run minting, so the listing's Id cannot be matched back to the identity the recording holds for that rule.

Cause 1 of 3: unfiltered ListNotificationRules page (two rules listed) diverges at NotificationRuleSummary.Id for BOTH entries. Fixture codestar-notifications-list-and-filter-rules, step task#6.
  AWS: Smithy: ListNotificationRules (POST /listNotificationRules, HTTP 200, @paginated inputToken=NextToken outputToken=NextToken items=NotificationRules pageSize=MaxResults) -> ListNotificationRulesResult.NotificationRules targets NotificationRuleBatch of NotificationRuleSummary{Id -> NotificationRuleId @length min 1 max 40, "The unique ID of the notification rule"; Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$}. The model constrains only the id's LENGTH, never its internal form, so for the form this is recorded behavior only: the recordings show AWS reporting, for each listed rule, the unique rule id it minted for that rule, identical to the id segment of that rule's ARN. No error is involved: both expected and actual status are 200 and no error code or message is returned on either side.
  Emulator: answers 200 with the right page and the right members, but the replay verdict is mismatch at $.NotificationRules[Arn="...notificationrule/ab795b6f-bef3-440d-8f04-49a65eac67f0"].Id and $.NotificationRules[Arn="...notificationrule/bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4"].Id, i.e. value_mismatch for two of two listed rules. The id it reports is the one state::new_rule_id minted for that rule in this run.
  Why: Id is the member a client uses to correlate a listing entry with a rule it already knows (and, with Arn, the key DescribeNotificationRule / Subscribe / UpdateNotificationRule / DeleteNotificationRule and the tag calls read back). A listing whose ids cannot be matched against the ids AWS issues breaks that correlation for every rule in the account, on every page.
  Fix: the id minting in crate::state::new_rule_id (and through it crate::state::notification_rule_arn), NOT the ListNotificationRules handler, which already returns the stored id unchanged; whatever the mint becomes, a summary's Id must stay byte-for-byte identical to the resource segment of that summary's Arn.
  Traces: codestar-notifications-list-and-filter-rules#task#6.

Cause 2 of 3: the same Id divergence on a FILTERED page returning a single rule. Fixture codestar-notifications-list-and-filter-rules, step task#7.
  AWS: as cause 1 — Smithy: NotificationRuleSummary.Id -> NotificationRuleId @length 1..40; filtering (Filters, OR within one name and AND across names) narrows WHICH rules are listed and changes nothing about the identity reported for a listed rule. HTTP 200, no error.
  Emulator: 200, the filter selects the correct rule (its Arn is matched by the comparator's selector), and the single entry's $.NotificationRules[Arn="...notificationrule/bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4"].Id still reports value_mismatch.
  Why: a filtered listing is how a client finds one rule by resource, event type, target address or creator; the one entry it gets back carries an id it cannot match against AWS's.
  Fix: same component as cause 1 (crate::state::new_rule_id). Nothing in the filter path (rule_satisfies / grouped_filters, src/ops/list_notification_rules.rs:86-98 and 145-154) is implicated — the right rule is selected.
  Traces: codestar-notifications-list-and-filter-rules#task#7.

Cause 3 of 3: the same Id divergence on the OTHER filtered page. Fixture codestar-notifications-list-and-filter-rules, step task#8.
  AWS: as cause 1/2 — recorded behavior only for the id's form; the model gives NotificationRuleId @length 1..40 and nothing else. HTTP 200, no error.
  Emulator: 200, correct rule selected, $.NotificationRules[Arn="...notificationrule/ab795b6f-bef3-440d-8f04-49a65eac67f0"].Id diverges (value_mismatch) for the single entry.
  Why: as cause 2 — every listing this service serves, filtered or unfiltered, reports a rule id a client cannot match.
  Fix: as cause 1/2.
  Traces: codestar-notifications-list-and-filter-rules#task#8.

Related open issue, which does NOT cover these three clusters' replay keys: 20261110T174500Z-codestar-notifications-rule-identifiers-diverge-createnotificationrule-arn-shape.md describes the same minting root cause and the CreateNotificationRule $.Arn cluster (codestar-notifications-rule-lifecycle#task#4, which the current replay reports as owned by it). Its listing-related Issue-Key lines name per-run ARNs from an earlier replay (notificationrule/2dcfbdba-... and notificationrule/7b85c7a4-...), so the current ListNotificationRules clusters replay with owned_by empty and are NOT gated by it. This issue carries their replay keys so a fixer re-runs task#6, task#7 and task#8. No status, error code or error message diverges in any cause.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 95 steps, 87 passed, 8 mismatched in 4 clusters, score 91.6%, emulator_up=true, bootstrap_signal=false, steps_blocked=0, steps_errored=0. Three of the four clusters are the ones filed here; the fourth (CreateNotificationRule $.Arn) reports owned_by the open rule-identifiers issue and is skipped.

All three clusters: service=codestar-notifications, operation=ListNotificationRules, verdict=mismatch, expected_status=200, actual_status=200, stub_path empty, owned_by empty, affected_count=1.

Cause 1 — fixture codestar-notifications-list-and-filter-rules, step task#6 (affected: codestar-notifications-list-and-filter-rules#task#6):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ab795b6f-bef3-440d-8f04-49a65eac67f0"].Id: value_mismatch (expected 'ab795b6f-bef3-440d-8f04-49a65eac67f0', got 'ab795b6f-bef3-440d-8f04-49a65eac67f0')
           $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4"].Id: value_mismatch (expected 'bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4', got 'bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4')
  divergence_key: service=codestar-notifications|ListNotificationRules|status=200->200|keys=$.NotificationRules[...ab795b6f...].Id,$.NotificationRules[...bb4dd50e...].Id
  Expected: each summary's Id equal to the recorded rule id for that rule (the id segment of that rule's ARN, in the form AWS mints).
  Actual: the per-run id minted by state::new_rule_id; the comparison fails for every entry of the page. The uuid values quoted above are per-run fill-ins, not the requirement — an earlier replay of the same steps carried 2dcfbdba-7c89-4c63-80dc-9fcea8f86cc3 and 7b85c7a4-d193-47e3-8263-5f8226b949f1 while the verdict was identical.

Cause 2 — same fixture, step task#7 (affected: codestar-notifications-list-and-filter-rules#task#7):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4"].Id: value_mismatch (expected 'bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4', got 'bb4dd50e-ea21-4a02-8d0f-c1560a7eadd4') — single filtered entry.

Cause 3 — same fixture, step task#8 (affected: codestar-notifications-list-and-filter-rules#task#8):
  detail: $.NotificationRules[Arn="arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/ab795b6f-bef3-440d-8f04-49a65eac67f0"].Id: value_mismatch (expected 'ab795b6f-bef3-440d-8f04-49a65eac67f0', got 'ab795b6f-bef3-440d-8f04-49a65eac67f0') — single filtered entry.

Code under the causes (same checkout the emulator is built from):
  services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/ops/list_notification_rules.rs:193-201 — NotificationRuleSummary { id: Some(rule.id.clone()), arn: Some(rule.arn.clone()) }.
  .../src/state.rs:562-586 — new_rule_id(sequence): sha256("notificationrule/<sequence>/<nanos>") laid out as "{8}-{4}-4{3}-8{3}-{12}".
  .../src/state.rs:366-374 — notification_rule_arn(region, id) = emulator_arn("codestar-notifications", region, "notificationrule/<id>").

Smithy model consulted (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): ListNotificationRules http POST /listNotificationRules code 200, @paginated(items=NotificationRules, pageSize=MaxResults), errors InvalidNextTokenException (400) and ValidationException (400) — neither is involved here; NotificationRuleSummary.Id -> NotificationRuleId @length min 1 max 40 (no @pattern, so the id's internal form is recorded behavior only); NotificationRuleSummary.Arn -> NotificationRuleArn @pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus against the booted emulator).
3. Cause 1: replay fixture codestar-notifications-list-and-filter-rules through step task#6 — an unfiltered ListNotificationRules (POST /listNotificationRules) after the fixture's rules have been created. The response is 200 with both rules present and correctly shaped, but $.NotificationRules[Arn="..."].Id reports value_mismatch for BOTH entries.
4. Cause 2: continue to step task#7 — a filtered ListNotificationRules returning one rule. 200, correct rule selected, the single entry's $.NotificationRules[Arn="..."].Id diverges.
5. Cause 3: continue to step task#8 — the other filtered listing. Same 200, same single-entry Id divergence.
6. Confirm the form, not one value, is at fault: replay the corpus twice. The rule ids reported change between runs (per-run minting by state::new_rule_id) while all three clusters diverge identically every time.
7. After a fix, re-replay codestar-notifications-list-and-filter-rules#task#6, #task#7 and #task#8; all three must pass, and each summary's Id must remain byte-for-byte identical to the resource segment of that summary's Arn.
