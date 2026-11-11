Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive
Issue-Key: cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive|_wait_log_events#12|Timeout|time.sleep(3)
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# cloudwatch__lambda__logs: a synchronous Invoke writes no log stream events into the function's log group

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the invocation's log stream events never become observable in /aws/lambda/<fn>.
AWS: a successful synchronous Invoke of a function whose role carries AWSLambdaBasicExecutionRole creates (if absent) /aws/lambda/<FunctionName>, creates a log stream named for the execution environment, and writes that invocation's events into it within seconds: 'START RequestId: <request id> Version: $LATEST', the handler's stdout lines, 'END RequestId: <request id>' and 'REPORT RequestId: <request id>\tDuration: ... ms\tBilled Duration: ... ms\tMemory Size: ... MB\tMax Memory Used: ... MB'. The fixed parts are the group name, the per-invocation stream and the START/END/REPORT framing; the request id, durations and memory figures are values of the run.
Emulator: the check never completes inside the suite window — the poll for the invocation's events in the function's log group never settles, so the positive 'an Invoke writes log stream events' assertion is never graded.
Why: this is the baseline Lambda observability contract every CloudWatch-side workflow depends on (DescribeLogStreams, GetLogEvents, metric filters, alarms). With an empty group a client sees a function that returns results but appears never to have logged anything, so nothing downstream of its logs can be built or tested.
Fix: the Lambda invoke path's log-delivery component that creates the log group/stream and PutLogEvents the START/stdout/END/REPORT records of each invocation. Open issue 20261105T111951Z-cloudwatch-lambda-logs-5-net-new-root-causes.md covers other checks of this service and does not claim this one.
Traces: cloudwatch__lambda__logs.lambda-invoke-writes-log-stream-events-positive.

## Evidence
Expected: after a successful synchronous Invoke, /aws/lambda/<fn> holds a stream with that invocation's START/stdout/END/REPORT events, readable within seconds.
Actual: 'Error: Check timed out after 60.0s' while polling the function's log group for those events.

## Steps to Reproduce
1. Create a role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole.
2. lambda.create_function(Runtime='python3.12', Handler='index.handler', Code={'ZipFile': <handler printing a marker>}) and wait for Active.
3. lambda.invoke(FunctionName=<fn>) and poll logs.describe_log_streams/filter_log_events on '/aws/lambda/<fn>' for the START/marker/REPORT events — none appear.
