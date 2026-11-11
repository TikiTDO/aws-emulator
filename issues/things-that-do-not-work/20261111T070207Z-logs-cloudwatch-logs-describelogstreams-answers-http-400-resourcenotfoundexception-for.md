Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target
Issue-Step: task#19
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: logs|DescribeLogStreams|2xx->4xx|$
Issue-Key: logs|DescribeLogStreams|2xx->4xx|$.logStreams
Issue-Related: 20261109T123745Z-logs-cloudwatch-logs-describelogstreams-refuses-an-existing-log-group.md
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# CloudWatch Logs DescribeLogStreams answers HTTP 400 ResourceNotFoundException for an existing Lambda log group

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogStreams

## Description
1 net-new cause, in the DescribeLogStreams handler's log-group resolution.

Cause 1 of 1: DescribeLogStreams refuses to enumerate the streams of a log group that exists, reporting the group as absent.

AWS: Smithy (com.amazonaws.cloudwatchlogs, logs-2014-03-28): DescribeLogStreams takes logGroupName (@length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$) or logGroupIdentifier (@length 1..2048), plus logStreamNamePrefix / orderBy / descending / nextToken / limit (@range 1..50), and returns DescribeLogStreamsResponse { logStreams: LogStreams, nextToken }. Each LogStream member carries logStreamName, creationTime, firstEventTimestamp, lastEventTimestamp, lastIngestionTime, uploadSequenceToken, arn and storedBytes (modelled @deprecated and always reported as 0 for log streams). The operation's only client-side error classes are InvalidParameterException (HTTP 400) for a malformed request and ResourceNotFoundException (HTTP 400, doc "The specified resource does not exist."), so ResourceNotFoundException is reserved for a log group that genuinely is not present. Neither applies to this request: the group is the /aws/lambda/<function-name> group implicitly created for the Lambda function that the ALB health checker invoked earlier in the same fixture, and the recorded response is HTTP 200 whose logStreams list holds the invocation's stream, with the ARN shaped arn:aws:logs:<region>:<account-id>:log-group:/aws/lambda/<function-name>:log-stream:<stream-name> and storedBytes 0 (the stream name, creation and event timestamps and upload sequence token are per-run values the recording does not assert). A log group that exists but has no stream matching logStreamNamePrefix is still HTTP 200 with an empty logStreams list, never a 400.

Emulator: answers HTTP 400 with body {"__type": "ResourceNotFoundException", "message": "The specified log group does not exist."} for that existing group; $.logStreams is absent entirely. The fixture's poll re-issued the call 19 times over the 180s cap and every attempt returned the same 400, so this is a steady refusal and not a propagation delay. In the error message, the fixed text is "The specified log group does not exist." and the error code is ResourceNotFoundException; there are no variable parts in it. The defect is that this error class is produced at all for a group that is present -- the correct response is HTTP 200 carrying logStreams, whose only variable parts are the stream names, the log-group name and account/region inside each stream ARN, and the timestamps, all taken from the request and the emulator's state.

Hypothesis: the log group created implicitly on behalf of a Lambda function is never registered in the Logs service's own state, so every Logs call that resolves a group by name misses it. Two already-open issues report the same class of 400 on sibling operations for this very fixture and are named here only as context -- 20261109T205846Z-logs-cloudwatch-logs-filterlogevents-answers-http-400-for-a.md (FilterLogEvents, task#20) and 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md (DeleteLogGroup, teardown#26). Neither carries the DescribeLogStreams replay key, so this operation's refusal is unowned and ungated.

Why: DescribeLogStreams is how a client discovers which streams a function wrote before reading them, and it is the standard readiness probe after invoking a function. A 400 ResourceNotFoundException tells an SDK caller the log group does not exist, which is false: a caller that treats 400 as fatal aborts a run whose Lambda actually executed and logged, one that retries on ResourceNotFoundException loops until its own budget expires (as the fixture's poll did), and any log-based verification or stream enumeration built on this call is impossible.

Fix: the DescribeLogStreams handler's log-group resolution and its 400-vs-200 decision, together with the state registration of auto-created /aws/lambda/<function-name> groups that it reads. A group that exists -- including one created implicitly for a Lambda function -- must resolve and return HTTP 200 with logStreams (empty when no stream matches the prefix) and nextToken paging, each stream carrying logStreamName, arn, creationTime, firstEventTimestamp, lastEventTimestamp, lastIngestionTime, uploadSequenceToken and storedBytes 0; ResourceNotFoundException must be reserved for a group that is genuinely absent.

Traces: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#19.

## Evidence
elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#19 -- DescribeLogStreams: status expected 200, got 400. divergence_key service=logs|DescribeLogStreams|status=200->400|keys=$,$.logStreams,status.

Expected (recording): HTTP 200, $.logStreams = [{arn: arn:aws:logs:<region>:<account-id>:log-group:/aws/lambda/<function-name>:log-stream:<stream-name>, logStreamName/creationTime/firstEventTimestamp/lastEventTimestamp/lastIngestionTime/uploadSequenceToken all shaped-not-asserted per-run values, storedBytes: 0}].

Actual (emulator): HTTP 400, body {"__type": "ResourceNotFoundException", "message": "The specified log group does not exist."}; $.logStreams missing. Replay reports "poll capped at 180s" and "poll: status 400, not the graded 200 after 19 attempt(s)" -- every one of the 19 attempts returned the same 400.

Message breakdown: error code ResourceNotFoundException and message text "The specified log group does not exist." are wholly fixed text with no request- or state-derived parts; they are wrong here only because the group exists. On the success path the variable parts are the log-group name and the account id/region embedded in each stream ARN (from the request and emulator state) and the stream name plus timestamps (generated per run, not asserted).

Context, already owned and NOT filed here: the same fixture's task#20 FilterLogEvents (owned by 20261109T205846Z-logs-cloudwatch-logs-filterlogevents-answers-http-400-for-a.md) and teardown#26 DeleteLogGroup (owned by 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md) answer 400 on the same group, which is why the shared log-group registration is the suspected root.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture elasticloadbalancing-alb-forwards-health-checks-to-lambda-target through task#18: an ALB target group forwards health checks to a Lambda target, which is invoked and writes a log line into its /aws/lambda/<function-name> log group.
3. At task#19 issue DescribeLogStreams against that log group (the fixture polls it until a stream appears).
4. Observe HTTP 400 with {"__type": "ResourceNotFoundException", "message": "The specified log group does not exist."} on every attempt, where the recording has HTTP 200 with the invocation's stream in logStreams.
