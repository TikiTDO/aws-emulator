Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected|run#30|AssertionError|assert sorted(map(_canon, parsed)) == sorted(map(_canon, _EXPECTED)), (
Issue-Commit: f886c6313691e264df098b8c73e16bb38687852c

# logs: the parseWAF transformer processor leaves httpRequest.headers as the raw name/value array instead of converting it to a header-name-keyed object

> _Note: this issue was identified against the code base at commit `f886c6313691e264df098b8c73e16bb38687852c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
One net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED in the replay report. Related open issues that do NOT carry this replay key: 20261029T182816Z-logs-transformers-are-not-applied-on-ingest-data.md and 20261111T030649Z-logs-5-net-new-causes-ingest-transformers-are.md (transformers not applied at all / other processors), and 20261031T171434Z-cloudwatch-logs-the-puttransformer-leading-parser-refusal-is.md (the leading-parser REFUSAL wording — which this check now verifies as correct, see below). This bundle has 1 cause; it must be closed before this issue is submitted.

Cause 1 of 1: parseWAF does not normalise the WAF record's httpRequest.headers.
Failing check: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected

The check's refusal half PASSES against this checkout: PutTransformer with transformerConfig [{parseJSON:{}}, {parseWAF:{}}] is correctly refused with InvalidParameterException and the message "Log processor parseWAF must be in the first position of the processor list", and GetTransformer afterwards returns an empty config. What diverges is the positive control that follows it.

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) PutTransformer takes logGroupIdentifier and transformerConfig: Processors @length 1..20, returns Unit, and models InvalidParameterException among its 400s. The ParseWAF processor's own documentation in that model states: "Use this processor to parse WAF vended logs, extract fields, and convert them into a JSON format. This processor always processes the entire log event message" and "If you use this processor, it must be the first processor in your transformer." The conversion AWS performs on a WAF vended record is to turn httpRequest.headers — which in the raw record is a LIST of objects each carrying a "name" and a "value" — into a single JSON OBJECT whose keys are the header names and whose values are the corresponding header values, leaving every other member of the record (timestamp, formatVersion, webaclId, terminatingRuleId, terminatingRuleType, action, httpSourceName, httpSourceId, ruleGroupList, rateBasedRuleList, nonTerminatingMatchingRules, and the remaining httpRequest members clientIp/country/uri/args/httpVersion/httpMethod/requestId) byte-equal to the input.
Emulator: with [{parseWAF:{}}] accepted as the group's transformer and a subscription using applyOnTransformedLogs=true, the delivered transformed event carries httpRequest.headers STILL as the raw array — e.g. [{"name":"Host","value":...},{"name":"User-Agent","value":...}] — where AWS delivers {"Host": ..., "User-Agent": ...}. Every other member matches, so the processor is running and passing the record through without performing the one conversion that distinguishes it from parseJSON.
Why: header-keyed access is the entire reason to run parseWAF rather than parseJSON. A downstream Logs Insights query, metric filter or subscriber addressing httpRequest.headers.Host (or any header by name) resolves to nothing against the emulator, and an index/metric built on the transformed shape silently produces no data — while the same configuration works on AWS.
Fix: the parseWAF processor implementation in the CloudWatch Logs ingest transformer pipeline — convert httpRequest.headers from the vended list-of-{name,value} form to a name-keyed object, and leave all other members untouched (the stored raw event must remain byte-identical, which the check also grades).
Traces: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected.

## Evidence
Cause 1 (verdict: mismatch) — logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected:
  File "check_logs_deep_logs_3_logacct_put_transformer_vended_parser_not_first_rejected.py", line 135, in run
  assert sorted(map(_canon, parsed)) == sorted(map(_canon, _EXPECTED))
  AssertionError: transformed events delivered from <log group> differ from the processor's contract:
    got      httpRequest.headers = [{"name": "Host", "value": "example.com"}, {"name": "User-Agent", "value": "sim2real/1.0"}]
    expected httpRequest.headers = {"Host": "example.com", "User-Agent": "sim2real/1.0"}
  Every other member of the delivered record is identical to the expectation: action 'ALLOW', formatVersion 1, httpSourceId, httpSourceName 'ALB', nonTerminatingMatchingRules [], rateBasedRuleList [], ruleGroupList [], terminatingRuleId 'Default_Action', terminatingRuleType 'REGULAR', timestamp, webaclId, and httpRequest.{clientIp,country,uri,args,httpVersion,httpMethod,requestId}.

Assertions in the same check that PASSED (so the refusal rule is already correct and must not regress): PutTransformer(transformerConfig=[{'parseJSON': {}}, {'parseWAF': {}}]) refused with code InvalidParameterException and message exactly 'Log processor parseWAF must be in the first position of the processor list'; GetTransformer after that refusal returns []; PutTransformer(transformerConfig=[{'parseWAF': {}}]) accepted and echoed back by GetTransformer.

Input event written by PutLogEvents (one event, message is the raw WAF record): {"timestamp": 1700000000000, "formatVersion": 1, "webaclId": ..., "terminatingRuleId": "Default_Action", "terminatingRuleType": "REGULAR", "action": "ALLOW", "httpSourceName": "ALB", "httpSourceId": ..., "ruleGroupList": [], "rateBasedRuleList": [], "nonTerminatingMatchingRules": [], "httpRequest": {"clientIp": "192.0.2.10", "country": "US", "headers": [{"name": "Host", "value": "example.com"}, {"name": "User-Agent", "value": "sim2real/1.0"}], "uri": "/a", "args": "q=1", "httpVersion": "HTTP/1.1", "httpMethod": "GET", "requestId": "r1"}}. The header names/values, webaclId and timestamp are the check's input data; the requirement is the list-to-object conversion, not those values.

Smithy citations: PutTransformer input {logGroupIdentifier: LogGroupIdentifier @length 1..2048 required, transformerConfig: Processors @length 1..20 required}, output Unit, errors [InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException (400), ServiceUnavailableException (500)]; ParseWAF documentation as quoted above.

## Steps to Reproduce
1. logs CreateLogGroup(G) + CreateLogStream(G, 'app').
2. Negative control that must keep passing: logs PutTransformer(logGroupIdentifier=G, transformerConfig=[{'parseJSON': {}}, {'parseWAF': {}}]) -> 400 InvalidParameterException 'Log processor parseWAF must be in the first position of the processor list'; logs GetTransformer(G) -> transformerConfig [].
3. logs PutTransformer(logGroupIdentifier=G, transformerConfig=[{'parseWAF': {}}]); poll GetTransformer(G) until it echoes the chain.
4. Create a Kinesis stream + logs-trusted delivery role and logs PutSubscriptionFilter(G, filterPattern='', destinationArn=<stream ARN>, roleArn=<role>, applyOnTransformedLogs=True).
5. logs PutLogEvents(G, 'app', [{timestamp: now_ms, message: <the raw WAF record above, with httpRequest.headers as a list of {name,value}>}]).
6. Drain the Kinesis stream and JSON-parse the delivered transformed message.
   Observed: httpRequest.headers is still the list of {name, value} objects.
   Expected: httpRequest.headers is an object keyed by header name ({"Host": "example.com", "User-Agent": "sim2real/1.0"}), with every other member of the record unchanged.
7. logs GetLogEvents(G, 'app') -> the STORED event must remain byte-identical to the raw record written in step 5.
