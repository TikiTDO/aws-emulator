Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
Issue-Key: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests|run#56|AssertionError|assert len(rgl) == 1, (
Issue-Commit: f886c6313691e264df098b8c73e16bb38687852c

# elbv2__logs__wafv2: WAF log records omit ruleGroupList entirely, so neither the terminating rule inside a referenced rule group nor the group's evaluation is attributable

> _Note: this issue was identified against the code base at commit `f886c6313691e264df098b8c73e16bb38687852c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
One net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED in the replay report; the open issue 20261109T035408Z-apigateway-logs-wafv2-the-waf-logging-configuration-check.md concerns an API Gateway-fronted WAF logging scenario that never produces a BLOCK record at all and does not carry this replay key. This bundle has 1 cause; it must be closed before the issue is submitted.

Cause 1 of 1: the WAF log record's ruleGroupList member is never emitted.
Failing check: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests

AWS: recorded behavior only — a WAF log record is vended JSON written to the CloudWatch Logs group named by wafv2 PutLoggingConfiguration, and its shape is not in the Smithy model (wafv2's LoggingConfiguration models only the destination, redaction and filter members). AWS's documented WAF log format carries, in every record produced by a web ACL that contains a RuleGroupReferenceStatement rule, a `ruleGroupList` array with one entry per evaluated rule-group reference. Each entry's `ruleGroupId` is the referenced rule group's identifier — the rule group's ARN for a customer-owned REGIONAL/CLOUDFRONT rule group — and its `terminatingRule` is an object {ruleId, action, ruleMatchDetails} naming the rule INSIDE the group that terminated the request (ruleId = that inner rule's Name, action = its configured action, e.g. BLOCK) when the group terminated, and null when the group was evaluated and terminated nothing. The emulator already gets the top-level attribution right in the same record: action BLOCK, terminatingRuleType 'GROUP' and terminatingRuleId = the web-ACL rule that references the group.
Emulator: `ruleGroupList` is absent from the record entirely (reads back as None) — on the BLOCKed request, where AWS carries one entry for the referenced group with terminatingRule {ruleId: <inner rule name>, action: 'BLOCK'}, and equally on the request the web ACL's default Allow let through, where AWS carries the same group listed with terminatingRule null to prove it WAS evaluated.
Why: ruleGroupList is the only place a WAF log record says WHICH rule inside a referenced rule group acted, and the only evidence that a rule group was evaluated at all for a request it did not terminate. Without it a consumer can see that "some rule group" blocked a request but never which rule, so rule-level tuning, false-positive triage and every dashboard/metric filter addressing ruleGroupList[].terminatingRule.ruleId produce nothing — and the allow-path assertion that the group was evaluated is unprovable.
Fix: the WAF log-record serializer on the wafv2 web-ACL evaluation path — populate ruleGroupList from the rule-group references the evaluation walked, with ruleGroupId set to the referenced group's ARN and terminatingRule set to the inner rule that terminated (ruleId + action) or null when none did.
Traces: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.

## Evidence
Cause 1 (verdict: mismatch) — elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests:
  File "check_elbv2__logs__wafv2_log_edge_producers_waf_log_records_rule_group_blocked_and_default_allowed_requests.py", line 398, in run
  assert len(rgl) == 1
  AssertionError: blocked record ruleGroupList is None; expected one entry for 'arn:aws:wafv2:<region>:<account>:regional/rulegroup/<rg name>/<rg id>'
  (the ARN in the message is per-run state, not part of the requirement.)

Setup established by the same run, all of which succeeded: a REGIONAL rule group (Capacity 20) with one inner rule {Name: 'blk-<marker>', Priority 0, Action Block, ByteMatchStatement EXACTLY on SingleHeader 'x-sim2real-block'}; a REGIONAL web ACL with DefaultAction Allow and two rules — {Name: 'cnt-<marker>', Priority 0, Action Count, ByteMatch STARTS_WITH on UriPath} and {Name: 'rg-<marker>', Priority 1, OverrideAction None, Statement RuleGroupReferenceStatement -> the rule group ARN}; the ACL associated with an internet-facing ALB; wafv2 PutLoggingConfiguration pointing at an 'aws-waf-logs-'-prefixed CloudWatch log group; then one request that matches the inner block rule and one that does not.

Assertions that PASSED on the blocked record (so the gap is the member, not the evaluation): action == 'BLOCK', terminatingRuleType == 'GROUP', terminatingRuleId == the web-ACL rule 'rg-<marker>' that references the group.

Expected on the BLOCKed record: ruleGroupList contains exactly one entry with ruleGroupId == the rule group's ARN and terminatingRule == {ruleId: 'blk-<marker>', action: 'BLOCK'}.
Expected on the ALLOWed record (graded immediately after, and equally unreachable): ruleGroupList contains the same group with terminatingRule null, alongside terminatingRuleId 'Default_Action' / terminatingRuleType 'REGULAR'.
Actual on both: the record has no ruleGroupList member at all.

## Steps to Reproduce
1. wafv2 CreateRuleGroup(Scope='REGIONAL', Capacity=20, Rules=[{Name:'blk-<marker>', Priority:0, Action:{Block:{}}, Statement:{ByteMatchStatement: SearchString=<marker>, PositionalConstraint='EXACTLY', FieldToMatch={SingleHeader:{Name:'x-sim2real-block'}}, TextTransformations:[{Priority:0,Type:'NONE'}]}}]).
2. wafv2 CreateWebACL(Scope='REGIONAL', DefaultAction={Allow:{}}, Rules=[{Name:'cnt-<marker>', Priority:0, Action:{Count:{}}, ByteMatch STARTS_WITH on UriPath}, {Name:'rg-<marker>', Priority:1, OverrideAction:{None:{}}, Statement:{RuleGroupReferenceStatement:{ARN:<rule group ARN>}}}]).
3. Create an internet-facing ALB and associate the web ACL with it; logs CreateLogGroup with an 'aws-waf-logs-' prefix and wafv2 PutLoggingConfiguration(ResourceArn=<ACL ARN>, LogDestinationConfigs=[<group ARN>]).
4. Send one HTTP request to the ALB carrying header x-sim2real-block: <marker> (blocked), and one without it (allowed by the default action), both on a URI path starting '/<marker>/'.
5. Read the delivered WAF records from the log group.
   Observed: each record has no 'ruleGroupList' member.
   Expected (blocked record): ruleGroupList == [{ruleGroupId: <rule group ARN>, terminatingRule: {ruleId: 'blk-<marker>', action: 'BLOCK', ...}}].
   Expected (allowed record): ruleGroupList == [{ruleGroupId: <rule group ARN>, terminatingRule: null, ...}].
