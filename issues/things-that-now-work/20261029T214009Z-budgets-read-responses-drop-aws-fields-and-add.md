Issue-Source: trace-sweep-divergence
Issue-Trace: budgets-account-wide-listings
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: budgets-account-wide-listings budgets-notification-subscriber-workflow budgets-cost-budget-lifecycle budgets-planned-budget-limits budgets-budget-action-crud-lifecycle
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|DescribeBudgetNotificationsForAccount|2xx->2xx|$.BudgetNotificationsForAccount[*]
Issue-Key: budgets|DescribeNotificationsForBudget|2xx->2xx|$.Notifications[*]
Issue-Key: budgets|DescribeBudgets|2xx->2xx|$.Budgets[*]
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.CreatedAt
Issue-Key: budgets|DescribeBudgetAction|2xx->2xx|$.Action.Subscribers[*].VerificationStatus
Issue-Key: budgets|DescribeBudgetPerformanceHistory|2xx->2xx|$.BudgetPerformanceHistory.BudgetedAndActualAmountsList[*].BudgetedAmount.Amount
Issue-Key: budgets|ListTagsForResource|2xx->2xx|$
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# budgets read responses drop AWS fields and add fields AWS omits (notifications, Budget.CreatedAt, Subscriber.VerificationStatus, Amount formatting, empty ListTagsForResource)

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBudgetNotificationsForAccount

## Description
Bundle of 5 net-new root causes in the budgets emulator's RESPONSE SERIALIZATION. Every cause answers HTTP 200 with the right operation semantics but a body whose member set or member rendering differs from AWS, so the divergence is invisible to the status code and only shows up in the payload. No open budgets issue covers them: 20261029T064018Z-budgets-describenotificationsforbudget-maintainability.md and 20261029T093141Z-budgets-describebudgetactionhistories-pagination.md are page-size/token concerns, 20261029T070543Z-budgets-tagresource-error-response.md is about the cross-account refusal on the tagging handlers (not the empty-result body), and the two 20261029T211259Z bundles are error-text/lifecycle findings from the behavioral suite. None of them carries the replay keys below.

Cause 1 of 5: every serialized Notification omits ConsecutiveBreachCount and adds ThresholdType.
AWS: recorded behavior only — Smithy's com.amazonaws.budgets#Notification declares NotificationType, ComparisonOperator, Threshold, ThresholdType and NotificationState, and has NO ConsecutiveBreachCount member, so the model cannot be cited either way. The recording shows AWS serializing a notification created with a PERCENTAGE threshold as exactly {NotificationType, ComparisonOperator, Threshold, NotificationState, ConsecutiveBreachCount} — i.e. it ALWAYS carries ConsecutiveBreachCount (0 while the notification has never breached, a value taken from service state) and it OMITS ThresholdType when the threshold type is the default PERCENTAGE. This holds for both account-wide and per-budget notification reads.
Emulator: serializes the mirror image — no ConsecutiveBreachCount member at all, plus an explicit "ThresholdType": "PERCENTAGE".
Why: a client that reads resp[...]['ConsecutiveBreachCount'] (the documented way to tell an alarm that has breached N evaluation periods in a row from a fresh one) gets a KeyError against the emulator, and a client that round-trips a described notification back into CreateNotification/UpdateNotification sends a ThresholdType AWS never echoed. Equality comparisons between a described notification and the notification the client created (the normal reconciliation idiom) fail on both members.
Fix: the shared Notification serializer in the budgets response layer, so every operation that emits a Notification (DescribeBudgetNotificationsForAccount, DescribeNotificationsForBudget and anything else embedding one) gets the same member set, with the breach counter sourced from the notification's stored state.
Traces: budgets-account-wide-listings#task#2 (DescribeBudgetNotificationsForAccount), budgets-notification-subscriber-workflow#task#3 and #task#9 (DescribeNotificationsForBudget).

Cause 2 of 5: serialized Budget objects have no CreatedAt member.
AWS: recorded behavior only — Smithy's com.amazonaws.budgets#Budget declares LastUpdatedTime but no CreatedAt, so again the model is silent; the recording shows AWS returning a CreatedAt timestamp (epoch seconds with millisecond fraction, the moment the budget was created, taken from service state) on the Budget in both DescribeBudget and DescribeBudgets. The emulator already emits the sibling LastUpdatedTime and HealthStatus.LastUpdatedTime, so only the creation stamp is missing. The recorded DescribeBudgets listing additionally carries NO CalculatedSpend on the listed budget, while the emulator adds CalculatedSpend {ActualSpend {Amount '0', Unit 'USD'}} there (DescribeBudget's CalculatedSpend matched, so the divergence is specific to the listing projection).
Emulator: the Budget it serializes has no CreatedAt key at all (DescribeBudget reports $.Budget.CreatedAt missing; DescribeBudgets reports the whole $.Budgets[0] object unequal), and the listing projection carries an extra CalculatedSpend.
Why: any client that sorts, ages out or audits budgets by creation time (the only distinction between a budget that was just created and one that has merely been updated) has no field to read, and a client that diffs a listed budget against a described one sees members appear and disappear depending on which call produced it.
Fix: the Budget serializer / budget state record in the budgets crate — persist the creation instant at CreateBudget and project it on every Budget emission, and make the DescribeBudgets listing projection emit the same member set AWS does. Note the concrete epoch values and the TimePeriod.Start in the recordings are per-run or clock-derived and are NOT requirements; the presence and shape of the member is.
Traces: budgets-cost-budget-lifecycle#task#2 and budgets-planned-budget-limits#task#2 (DescribeBudget), budgets-account-wide-listings#task#3 and budgets-cost-budget-lifecycle#task#3 (DescribeBudgets).

Cause 3 of 5: a budget action's Subscribers lose VerificationStatus.
AWS: recorded behavior only — Smithy's com.amazonaws.budgets#Subscriber declares only SubscriptionType and Address, so the model is silent; the recording shows every Subscriber inside an Action carrying VerificationStatus, with value 'PENDING' for an EMAIL subscriber whose address has not been confirmed (the value comes from service state, not from the request).
Emulator: serializes {SubscriptionType, Address} only, so $.Action.Subscribers[0].VerificationStatus is absent on DescribeBudgetAction (and on the Action embedded in action-history entries).
Why: a client that waits for a subscriber's address to be verified before relying on the action's alerts has nothing to poll, and the subscriber object cannot be compared against the one AWS returns.
Fix: the Subscriber serializer used by the budget-action responses, sourcing the verification state from the stored subscriber record.
Traces: budgets-budget-action-crud-lifecycle#task#3 and #task#5, budgets-budget-action-execute-rejected-and-history#task#5 (DescribeBudgetAction).

Cause 4 of 5: monetary Amount strings are re-rendered through a float instead of echoed.
AWS: Smithy: com.amazonaws.budgets#Spend.Amount is a required string (NumericValue, @length min 1, @pattern ^([0-9]*\.)?[0-9]+$) — i.e. the amount is a DECIMAL STRING, deliberately not a number, so its textual form is part of the value. The recording shows AWS echoing the amount exactly as the client supplied it: a budget whose limit was given as '250' is reported in DescribeBudgetPerformanceHistory as BudgetedAmount.Amount == '250', and one given as '300.0' is reported as '300.0'.
Emulator: parses the amount into a floating-point number and re-renders it, so '250' comes back as '250.0'.
Why: Amount is a string precisely so that clients can compare and hash it, assert on it in snapshot tests, and keep decimal precision; a value that silently changes its spelling breaks equality checks against the value the client wrote, and a float round-trip is a precision hazard for large or many-decimal amounts.
Fix: the budgets state layer should store the Amount string verbatim and the Spend serializer should emit it unchanged, rather than normalizing through a numeric type.
Traces: budgets-cost-budget-lifecycle#task#7 and budgets-planned-budget-limits#task#4 (DescribeBudgetPerformanceHistory).

Cause 5 of 5: ListTagsForResource emits an empty ResourceTags list where AWS omits the member.
AWS: Smithy: ListTagsForResource output has a single OPTIONAL member ResourceTags (ResourceTagList, @length min 0 max 200). The recording shows that for a resource with no tags AWS answers HTTP 200 with the empty JSON document {} — the member is omitted, not serialized as [].
Emulator: answers 200 with {"ResourceTags": []}.
Why: this is the service's own convention for 'nothing to report' and clients branch on it; code written as `if 'ResourceTags' in resp` takes the has-tags path against the emulator and the no-tags path against AWS, and response-equality/snapshot checks diverge on every untagged resource.
Fix: the ListTagsForResource response serializer (and the general rule for optional list members in the budgets serializer) must skip an empty collection instead of emitting it.
Traces: budgets-cost-budget-lifecycle#task#9 (ListTagsForResource).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 69/89 steps passed, 20 mismatched, score 77.5%, emulator_up=true. All 5 causes below are verdict=mismatch with expected_status == actual_status == 200 — the bodies diverge, not the statuses.

Cause 1 — fixture budgets-account-wide-listings step task#2, DescribeBudgetNotificationsForAccount:
  $.BudgetNotificationsForAccount[0]: expected {'BudgetName': <name>, 'Notifications': [{'ComparisonOperator': 'GREATER_THAN', 'ConsecutiveBreachCount': 0, 'NotificationState': 'OK', 'NotificationType': 'ACTUAL', 'Threshold': 70.0}]}, got {'BudgetName': <name>, 'Notifications': [{'NotificationType': 'ACTUAL', 'ComparisonOperator': 'GREATER_THAN', 'Threshold': 70.0, 'ThresholdType': 'PERCENTAGE', 'NotificationState': 'OK'}]} — ConsecutiveBreachCount absent, ThresholdType extra (budget name and threshold are per-run fixture values).
  Same diff shape at budgets-notification-subscriber-workflow task#3 and task#9 (DescribeNotificationsForBudget): $.Notifications[0] expected {..., 'ConsecutiveBreachCount': 0, ...}, got {..., 'ThresholdType': 'PERCENTAGE', ...}.

Cause 2 — fixture budgets-cost-budget-lifecycle step task#2, DescribeBudget:
  $.Budget.CreatedAt: missing (expected a creation timestamp, got None). Identical at budgets-planned-budget-limits#task#2.
  fixture budgets-account-wide-listings step task#3, DescribeBudgets: $.Budgets[0] unequal as a whole — the expected object carries 'CreatedAt' and carries NO 'CalculatedSpend'; the emulator's object has no 'CreatedAt' and adds 'CalculatedSpend': {'ActualSpend': {'Amount': '0', 'Unit': 'USD'}}. (The expected/actual TimePeriod.Start and the LastUpdatedTime values also differ; those are clock-derived per-run values and are not part of this finding.) Identical at budgets-cost-budget-lifecycle#task#3.

Cause 3 — fixture budgets-budget-action-crud-lifecycle step task#3, DescribeBudgetAction:
  $.Action.Subscribers[0].VerificationStatus: missing (expected 'PENDING', got None). Identical at budgets-budget-action-crud-lifecycle#task#5 and budgets-budget-action-execute-rejected-and-history#task#5.

Cause 4 — fixture budgets-cost-budget-lifecycle step task#7, DescribeBudgetPerformanceHistory:
  $.BudgetPerformanceHistory.BudgetedAndActualAmountsList[0].BudgetedAmount.Amount: expected '250', got '250.0'. Identical at budgets-planned-budget-limits#task#4. In the same corpus an amount supplied as '300.0' round-trips as '300.0', which is what shows the emulator is reformatting rather than echoing.

Cause 5 — fixture budgets-cost-budget-lifecycle step task#9, ListTagsForResource:
  $: expected {} (no ResourceTags key), got {'ResourceTags': []}.

## Steps to Reproduce
All against the emulator's budgets endpoint with account 000000000000; replay the fixtures/steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids for the exact recorded requests.

Cause 1: CreateBudget B (COST, MONTHLY, BudgetLimit); CreateNotification on B with Notification={NotificationType: ACTUAL, ComparisonOperator: GREATER_THAN, Threshold: 70.0, ThresholdType: PERCENTAGE} and one EMAIL subscriber; then call DescribeBudgetNotificationsForAccount(AccountId) and DescribeNotificationsForBudget(AccountId, BudgetName=B) -> inspect each returned Notification: no ConsecutiveBreachCount, extra ThresholdType.

Cause 2: CreateBudget B; DescribeBudget(AccountId, BudgetName=B) -> $.Budget has no CreatedAt. Then DescribeBudgets(AccountId) -> the listed budget has no CreatedAt and an extra CalculatedSpend.

Cause 3: CreateBudget B; CreateBudgetAction on B (APPLY_IAM_POLICY, MANUAL, ActionThreshold 80 PERCENTAGE, an assumable ExecutionRoleArn, Subscribers=[{SubscriptionType: EMAIL, Address: <addr>}]); DescribeBudgetAction(AccountId, BudgetName=B, ActionId) -> $.Action.Subscribers[0] has no VerificationStatus.

Cause 4: CreateBudget B with an amount whose string form is not float-normal, e.g. BudgetLimit/PlannedBudgetLimits Amount '250'; DescribeBudgetPerformanceHistory(AccountId, BudgetName=B) -> BudgetedAmount.Amount comes back '250.0'.

Cause 5: CreateBudget B; ListTagsForResource(ResourceARN='arn:aws:budgets::000000000000:budget/B') with no tags on it -> body is {"ResourceTags": []} instead of {}.
