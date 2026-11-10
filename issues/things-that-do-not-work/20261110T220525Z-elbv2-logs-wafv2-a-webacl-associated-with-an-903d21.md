Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
Issue-Key: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests|_poll_until#49|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# elbv2__logs__wafv2: a WebACL associated with an ALB writes no WAF log record for either a rule-group-blocked or a default-allowed request

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a WebACL associated with an Application Load Balancer and configured with a CloudWatch Logs destination produces no log record at all — neither for a request its rule group BLOCKED nor for a request its default action ALLOWED.
AWS: Smithy (wafv2) AssociateWebACL (ResourceArn = the ALB ARN), PutLoggingConfiguration (LoggingConfiguration.{ResourceArn, LogDestinationConfigs}) and the aws-waf-logs-* log-group naming requirement the destination must satisfy; Smithy (logs) FilterLogEvents/GetLogEvents for reading the destination. AWS behaviour: every request the WebACL evaluates for the associated resource is written as one JSON log record into the destination, carrying at minimum the timestamp, webaclId, terminatingRuleId/terminatingRuleType, action (BLOCK or ALLOW), and the httpRequest it describes; a request terminated by a rule inside a referenced rule group records that rule group as the terminating rule, and a request that matches nothing records the WebACL's default action. The requirement is one record per evaluated request with the terminating rule and action it reflects.
Emulator: nothing is ever written. Replay: TimeoutError "WAF log records for both graded requests in aws-waf-logs-sim2real-lel-<id>: the run budget ended first; not graded" — the log group's suffix is a per-run value; the defect is the absence of both records over a 900 s poll.
Why: WAF logs are the only record of what the firewall in front of an application decided. With none, a client cannot tell whether a request was blocked, by which rule, or whether the WebACL was consulted at all — and an unenforced WebACL is indistinguishable from an enforced one with missing logs. Security auditing, rule tuning and incident response on the edge all have nothing to read.
Fix: the WAFv2 logging component — the WebACL request-evaluation path for an associated ELBv2 resource must emit one record per evaluated request to each configured LogDestinationConfigs destination, populating webaclId, terminatingRuleId/terminatingRuleType (including the referenced rule group for a rule-group termination), action and httpRequest.
Related open filings that do NOT carry this replay key: 20261104T094656Z-elbv2-logs-wafv2-a-webacl-associated-with-an.md and 20261109T143515Z-elbv2-logs-wafv2-the-waf-protected-alb-s-endpoint-host-is.md cover the earlier non-enforcement and unserved-endpoint states under this heading; the suite did not mark this check OWNED, so it survives as a live failure.
Traces: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading elbv2__logs__wafv2 0/1 (0%), 1 failed.

[error] elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
  File "check_elbv2__logs__wafv2_log_edge_producers_waf_log_records_rule_group_blocked_and_default_allowed_requests.py", line 381, in run -> got = _poll_until(ctx, _graded, lambda g: all(g.values()), timeout_s=900, interval_s=2, ...)
  File "...", line 71, in _poll_until -> raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: WAF log records for both graded requests in aws-waf-logs-sim2real-lel-c6af7701d3b9: the run budget ended first; not graded

Expected (AWS): two records in the aws-waf-logs-* destination — one for the request terminated by the referenced rule group (action BLOCK, terminatingRuleType RULE_GROUP, the rule group named) and one for the request that fell through to the WebACL's default action (action ALLOW) — each carrying webaclId and the httpRequest.
Actual (emulator): no record of either request appears within 900 s.

Smithy consulted: wafv2 AssociateWebACL {WebACLArn, ResourceArn}; PutLoggingConfiguration input LoggingConfiguration {ResourceArn, LogDestinationConfigs, RedactedFields, LoggingFilter}; logs FilterLogEvents for the destination group. The WAF log record's fields (timestamp, webaclId, terminatingRuleId/terminatingRuleType, action, httpRequest) are recorded behavior plus the WAF logging documentation.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Create an ALB with a listener that serves a 200, create a CloudWatch Logs group whose name starts with aws-waf-logs-, and create a WAFv2 regional WebACL with DefaultAction ALLOW that references a rule group which blocks a distinguishable request.
3. AssociateWebACL(WebACLArn=<acl>, ResourceArn=<alb arn>) and PutLoggingConfiguration(LoggingConfiguration={ResourceArn:<acl>, LogDestinationConfigs:[<log group arn>]}).
4. Send one request that the rule group blocks and one that matches nothing.
5. Read the log group. Expected: one record per request with webaclId, terminatingRuleId/terminatingRuleType, action BLOCK/ALLOW and the httpRequest. Actual: no records at all.
6. Re-replay the id in trace_ids to confirm the fix.
