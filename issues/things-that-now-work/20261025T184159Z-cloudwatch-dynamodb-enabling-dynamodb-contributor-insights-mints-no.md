Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.obs-dynamodb-2-contributor-insights-hot-key-closed-loop cloudwatch__dynamodb.obs-dynamodb-2-throttled-keys-mode-accessed-report-rejected
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-2-contributor-insights-hot-key-closed-loop|run#16|AssertionError|assert kinds[table] == ["PKC", "PKT"], (
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-2-throttled-keys-mode-accessed-report-rejected|run#14|AssertionError|assert len(rules) == 1, f"THROTTLED_KEYS on a hash-only table minted {sorted(rules)!r}"
Issue-Commit: 8872a34b239da6f35a39fa56ec21084188bccf39

# cloudwatch__dynamodb: enabling DynamoDB Contributor Insights mints no CloudWatch insight rules, so DescribeInsightRules/GetInsightRuleReport can never see the table's PKC/PKT rules

> _Note: this issue was identified against the code base at commit `8872a34b239da6f35a39fa56ec21084188bccf39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateContributorInsights

## Description
One root cause (Cause 1 of 1).

AWS: Smithy model — dynamodb UpdateContributorInsights{TableName (required), IndexName, ContributorInsightsAction (required), ContributorInsightsMode (enum com.amazonaws.dynamodb#ContributorInsightsMode = ACCESSED_AND_THROTTLED_KEYS | THROTTLED_KEYS)} returns ContributorInsightsStatus (ENABLING | ENABLED | DISABLING | DISABLED | FAILED) and echoes ContributorInsightsMode. The model says nothing about the CloudWatch side, so for that the oracle is recorded behavior (measured us-west-2, 2026-10-01, as the checks' own comments record): ENABLE on a table causes DynamoDB to CREATE CloudWatch Contributor Insights rules on the caller's account, discoverable through cloudwatch DescribeInsightRules and reportable through cloudwatch GetInsightRuleReport. Observed invariants: the rule names are DynamoDB-minted as DynamoDBContributorInsights-<PKC|PKT|SKC|SKT>-<TableName>-<epoch-millis>; PKC ranks most-accessed partition keys (ValueOf ConsumedThroughputUnits) and PKT most-throttled partition keys (ValueOf ThrottleCount); the PKC and PKT rules created by one ENABLE share a single epoch-millis suffix; a hash-only (no sort key) table gets no SKC/SKT rules; mode ACCESSED_AND_THROTTLED_KEYS mints exactly PKC + PKT, mode THROTTLED_KEYS mints exactly PKT; GetInsightRuleReport on such a rule returns KeyLabels == ["PartitionKey"] with per-key Contributors; DISABLE deletes the rules, after which GetInsightRuleReport on the old name is refused InvalidParameterValue with message "RULE_NAME: <ruleName>" (fixed text "RULE_NAME: " plus the requested rule name from the request).

Emulator: UpdateContributorInsights accepts ENABLE, answers ContributorInsightsStatus "ENABLING", echoes the requested ContributorInsightsMode, and DescribeContributorInsights goes on to report ENABLED — but NO CloudWatch insight rule is ever created. cloudwatch DescribeInsightRules returns no rule whose name contains "-<TableName>-", for either mode:
  - with ContributorInsightsMode=ACCESSED_AND_THROTTLED_KEYS the set of rule kinds minted for the table is [] where it must be [PKC, PKT]  ("AssertionError: ACCESSED_AND_THROTTLED_KEYS on a hash-only table minted [], expected PKC + PKT")
  - with ContributorInsightsMode=THROTTLED_KEYS the rule set is [] where exactly one (PKT) rule must exist  ("AssertionError: THROTTLED_KEYS on a hash-only table minted []")
Because nothing is minted, every downstream CloudWatch assertion is unreachable: no GetInsightRuleReport over the table's reads, no ranked contributors, no DISABLE-deletes-the-rules step and therefore no InvalidParameterValue "RULE_NAME: <ruleName>" refusal on the deleted rule.

Why: a client wiring the documented DynamoDB hot-key workflow — enable Contributor Insights, then read the ranked report from CloudWatch to find the hot or throttled partition key — gets a table that claims ENABLED while the CloudWatch side is empty. DescribeInsightRules lists nothing for the table, GetInsightRuleReport has no rule name to call, and dashboards/automation keyed on DynamoDBContributorInsights-<PKC|PKT>-<table>-<millis> never find their rule. The status is a false positive: the control plane reports the feature on while the observability it exists to provide does not exist, so an operator concludes there is no hot key rather than that there is no data.

Fix: the dynamodb UpdateContributorInsights handler and the state it writes, which must project into the cloudwatch service's insight-rule store (the store DescribeInsightRules / GetInsightRuleReport read): on ENABLE create the mode's rules — PKC + PKT for ACCESSED_AND_THROTTLED_KEYS, PKT only for THROTTLED_KEYS, plus SKC/SKT only when the table has a sort key — named DynamoDBContributorInsights-<kind>-<TableName>-<epoch-millis> with one shared suffix per ENABLE, aggregating ConsumedThroughputUnits (PKC) and ThrottleCount (PKT) per partition key with KeyLabels ["PartitionKey"]; on DISABLE delete exactly that table's rules and leave other tables' rules untouched, so GetInsightRuleReport on a deleted name is refused InvalidParameterValue with message "RULE_NAME: " + the requested rule name.

Traces: cloudwatch__dynamodb.obs-dynamodb-2-contributor-insights-hot-key-closed-loop, cloudwatch__dynamodb.obs-dynamodb-2-throttled-keys-mode-accessed-report-rejected. Related open issues that do NOT cover this: 20261025T070825Z-cloudwatch-contributor-insights-reporting-is-inert-a-rule-over.md (a caller-created rule over a log group yields no report — that is CloudWatch's own report path, not DynamoDB minting rules); 20261025T020208Z-dynamodb-updatecontributorinsights-stub.md and 20261025T024002Z-dynamodb-describecontributorinsights-stub.md (stub refusals; here both operations answer).

## Evidence
Check cloudwatch__dynamodb.obs-dynamodb-2-contributor-insights-hot-key-closed-loop
  Expected (recorded AWS): after UpdateContributorInsights(TableName=t, ContributorInsightsAction=ENABLE, ContributorInsightsMode=ACCESSED_AND_THROTTLED_KEYS) on a hash-only table, cloudwatch DescribeInsightRules lists exactly two rules for t, named DynamoDBContributorInsights-PKC-<t>-<millis> and DynamoDBContributorInsights-PKT-<t>-<millis> sharing one <millis> suffix (kinds [PKC, PKT]; no SKC/SKT for a hash-only table).
  Actual (emulator): kinds minted == []
    AssertionError: ACCESSED_AND_THROTTLED_KEYS on a hash-only table minted [], expected PKC + PKT
  Preceding steps succeeded: UpdateContributorInsights returned ContributorInsightsStatus "ENABLING" and echoed ContributorInsightsMode; DescribeContributorInsights reached "ENABLED".

Check cloudwatch__dynamodb.obs-dynamodb-2-throttled-keys-mode-accessed-report-rejected
  Expected (recorded AWS): ContributorInsightsMode=THROTTLED_KEYS mints exactly one rule for the table, DynamoDBContributorInsights-PKT-<t>-<millis> (len(rules) == 1).
  Actual (emulator): AssertionError: THROTTLED_KEYS on a hash-only table minted []

Unreachable because of the same cause (never exercised): GetInsightRuleReport with KeyLabels == ["PartitionKey"] and per-partition-key Contributors (90 reads on one key -> Sum 90, ten other keys -> Sum 1 each, descending); DISABLE removing the table's rules while a still-enabled control table keeps its own; GetInsightRuleReport on the deleted rule refused InvalidParameterValue with message "RULE_NAME: <ruleName>" (fixed text "RULE_NAME: ", value = the rule name from the request).

## Steps to Reproduce
1. dynamodb CreateTable a hash-only PAY_PER_REQUEST table t (KeySchema [{pk, HASH}]) and wait for ACTIVE.
2. dynamodb UpdateContributorInsights(TableName=t, ContributorInsightsAction=ENABLE, ContributorInsightsMode=ACCESSED_AND_THROTTLED_KEYS). Note it answers ContributorInsightsStatus ENABLING and DescribeContributorInsights later reports ENABLED.
3. cloudwatch DescribeInsightRules (paginating NextToken) and filter InsightRules for names containing "-<t>-". Observe NO rules, where AWS has DynamoDBContributorInsights-PKC-<t>-<millis> and DynamoDBContributorInsights-PKT-<t>-<millis>.
4. Repeat steps 1-3 on a second table with ContributorInsightsMode=THROTTLED_KEYS. Observe NO rules, where AWS has exactly DynamoDBContributorInsights-PKT-<t>-<millis>.
5. (Blocked by the above) Issue consistent GetItem reads against one partition key, then cloudwatch GetInsightRuleReport(RuleName=the PKC rule, Period=60, MaxContributorCount=20, OrderBy="Sum") and check KeyLabels == ["PartitionKey"] and the per-key Sums; then UpdateContributorInsights(DISABLE) and check the rules are gone and GetInsightRuleReport on the old name is refused InvalidParameterValue "RULE_NAME: <ruleName>".
6. Re-replay: cloudwatch__dynamodb.obs-dynamodb-2-contributor-insights-hot-key-closed-loop, cloudwatch__dynamodb.obs-dynamodb-2-throttled-keys-mode-accessed-report-rejected.
