Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-contributor-insights-error-cases
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-contributor-insights-error-cases monitoring-contributor-insights-and-managed-rules cwp-describe-insight-rules-pagination cloudwatch-contributor-insights-rule-lifecycle cloudwatch-managed-insight-rules-for-resource cloudwatch-managed-insight-rules-lifecycle cloudwatch-managed-insight-rules-workflow
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|PutInsightRule|4xx->4xx|$.message
Issue-Key: monitoring|GetInsightRuleReport|4xx->4xx|$.__type
Issue-Key: monitoring|GetInsightRuleReport|4xx->4xx|$.message
Issue-Key: monitoring|GetInsightRuleReport|4xx->4xx|$headers.x-amzn-query-error
Issue-Key: monitoring|GetInsightRuleReport|4xx->4xx|status
Issue-Key: monitoring|DescribeInsightRules|4xx->4xx|$.message
Issue-Key: monitoring|DeleteInsightRules|2xx->2xx|$.Failures[*]
Issue-Key: monitoring|DescribeInsightRules|4xx->4xx|$.__type
Issue-Key: monitoring|DescribeInsightRules|4xx->4xx|$headers.x-amzn-query-error
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch Contributor Insights error paths use emulator-authored codes and messages

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInsightRule

## Description
5 causes, one shared fault: the Contributor Insights refusal paths are authored by the emulator instead of reproducing AWS's error class and message text for the class, and one of them reports a batch failure where AWS reports success.

Cause 1 of 5: PutInsightRule with a RuleDefinition that is not a parseable CloudWatchLogRule body. AWS: 400 InvalidParameterValueException (Smithy: PutInsightRule errors include InvalidParameterValueException, awsQueryError code InvalidParameterValue) with the message 'INVALID_RULE_BODY: The RuleBody could not be parsed.' — entirely fixed text for this class; the Contributor Insights error messages are '<REASON_CODE>: <sentence>' pairs. Emulator: 400 with its own prose 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1'. Why: a client branching on the reason code prefix (INVALID_RULE_BODY) cannot distinguish a malformed body from a bad rule name or an over-limit rule, and surfaced text does not match AWS docs. Fix: PutInsightRule's validation in the CloudWatch crate. Trace: cloudwatch-contributor-insights-error-cases#task#1.

Cause 2 of 5: GetInsightRuleReport for a rule name that does not exist. AWS: 400 with __type 'com.amazon.coral.service#InvalidParameterValueException', header 'x-amzn-query-error: InvalidParameterValue;Sender' and the message 'RULE_NAME: <the RuleName from the request>' — fixed prefix 'RULE_NAME: ' plus the requested name. (Smithy models ResourceNotFoundException on this operation too, but the recording shows the service classifying an unknown rule name as InvalidParameterValue here.) Emulator: 404 with __type 'ResourceNotFoundException', header 'ResourceNotFoundException;Sender' and NO message member at all. Why: the status class changes from 4xx-sender-validation to not-found and the message is absent, so a client cannot tell which rule name was rejected and retry logic keyed on 400-vs-404 diverges. Fix: GetInsightRuleReport's existence check and error mapping. Trace: cloudwatch-contributor-insights-error-cases#task#3.

Cause 3 of 5: DescribeInsightRules with a NextToken the service did not issue. AWS: 400 with the message 'INVALID_TOKEN: Unsupported token version.' — fixed text, no echo of the token. Emulator: 400 with 'NextToken is not a token this service issued'. Why: clients and tests that detect an expired/foreign pagination token by this text do not match; the Contributor Insights reason-code form is lost. Fix: the DescribeInsightRules pagination-token validation. Trace: cloudwatch-contributor-insights-error-cases#task#4.

Cause 4 of 5: DeleteInsightRules naming a rule that does not exist. AWS: 200 with NO Failures entry for that name — the delete of an absent rule is not reported as a failure on this path. Smithy: DeleteInsightRules output Failures targets BatchFailures (optional). Emulator: 200 but with a synthesized Failures entry (FailureCode 'RULE_NAME', ExceptionType 'ResourceNotFoundException', FailureDescription 'Rule <name> does not exist.'). Why: idempotent teardown code that treats a non-empty Failures array as an error aborts on a clean delete. Fix: the DeleteInsightRules handler's batch-result assembly. Trace: cloudwatch-contributor-insights-error-cases#teardown#5.

Cause 5 of 5: DescribeInsightRules with MaxResults 0. AWS: 400 with __type 'com.amazon.coral.service#InvalidParameterValueException', header 'InvalidParameterValue;Sender' and the message 'INVALID_MAX_RESULTS: MaxResults must be greater than 0.' — fixed text for the class. Smithy: DescribeInsightRules.MaxResults targets InsightRuleMaxResults @range(min 1, max 500). Emulator: 400 with __type 'ValidationException', no query-error header, and the generic constraint message "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". Why: the service distinguishes a service-specific InvalidParameterValue from a request-shape ValidationError here; a client classifying on that code mis-routes the failure. Fix: DescribeInsightRules' MaxResults validation (it must run as an operation-level InvalidParameterValue check, not as generic range enforcement). Trace: cwp-describe-insight-rules-pagination#task#7.

## Evidence
Replay (simparity-wire-aws-v1), 5 clusters.
1. PutInsightRule, cloudwatch-contributor-insights-error-cases#task#1: $.message expected 'INVALID_RULE_BODY: The RuleBody could not be parsed.', got 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1' (also monitoring-contributor-insights-and-managed-rules#task#9).
2. GetInsightRuleReport, cloudwatch-contributor-insights-error-cases#task#3: status expected 400, got 404; $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'ResourceNotFoundException'; $.message expected 'RULE_NAME: <rule name>', got None; header expected 'InvalidParameterValue;Sender', got 'ResourceNotFoundException;Sender'.
3. DescribeInsightRules, cloudwatch-contributor-insights-error-cases#task#4: $.message expected 'INVALID_TOKEN: Unsupported token version.', got 'NextToken is not a token this service issued' (also cwp-describe-insight-rules-pagination#task#6).
4. DeleteInsightRules, cloudwatch-contributor-insights-error-cases#teardown#5: $.Failures[0] extra (expected None, got {'FailureResource': '<rule>', 'ExceptionType': 'ResourceNotFoundException', 'FailureCode': 'RULE_NAME', 'FailureDescription': 'Rule <rule> does not exist.'}) — 6 affected steps including cloudwatch-managed-insight-rules-lifecycle#teardown#3 and monitoring-contributor-insights-and-managed-rules#teardown#12.
5. DescribeInsightRules, cwp-describe-insight-rules-pagination#task#7: $.__type expected 'com.amazon.coral.service#InvalidParameterValueException', got 'ValidationException'; $.message expected 'INVALID_MAX_RESULTS: MaxResults must be greater than 0.', got "1 validation error detected: Value '0' at 'maxResults' …"; header expected 'InvalidParameterValue;Sender', got None.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutInsightRule with a RuleDefinition that is not a CloudWatchLogRule body (cloudwatch-contributor-insights-error-cases#task#1) and read $.message.
3. GetInsightRuleReport for a rule name that was never created (same fixture, task#3) and read the status, __type, message and x-amzn-query-error header.
4. DescribeInsightRules with NextToken='not-a-real-pagination-token' (task#4) and with MaxResults=0 (cwp-describe-insight-rules-pagination#task#7).
5. DeleteInsightRules naming an absent rule (cloudwatch-contributor-insights-error-cases#teardown#5) and inspect Failures.
