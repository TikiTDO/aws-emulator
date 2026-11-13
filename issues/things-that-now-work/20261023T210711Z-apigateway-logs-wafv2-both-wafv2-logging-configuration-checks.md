Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected
Issue-Key: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__logs__wafv2: both WAFv2 logging-configuration checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
Cause 1 of 1: the checks drive traffic at a deployed REST API stage protected by a WAFv2 web ACL, and that request never returns.
AWS: a request blocked by the associated web ACL is answered at the stage (403 by default) and a corresponding log record is delivered to the configured destination with the declared fields redacted; PutLoggingConfiguration naming a log group without the `aws-waf-logs-` prefix is refused. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives from the stage; both checks exhaust the harness budget and are reported as "Check timed out after 60.0s", so neither the blocked-request record nor the prefix refusal is ever reached.
Why: WAFv2 logging against an API Gateway stage cannot be observed at all — a client sees a hang where AWS answers and logs.
Fix: the execute-api data-plane request path (web-ACL evaluation at the stage) — it must always produce a response.
Traces: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields, apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected.

## Evidence
Detail for both checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Deploy a REST API stage, create a WAFv2 REGIONAL web ACL with a blocking rule and associate it with the stage; configure logging to an `aws-waf-logs-` log group with redacted fields.
2. Send a request that the rule blocks and observe that the stage never responds within 60 s.
3. Or replay the check ids in trace_ids.
