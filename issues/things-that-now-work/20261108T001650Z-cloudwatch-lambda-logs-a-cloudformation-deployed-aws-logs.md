Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: f42419e01630977583bc09726b6f41b0d8951344

# cloudwatch__lambda__logs: a CloudFormation-deployed AWS::Logs::SubscriptionFilter with a Lambda destination never delivers a matching log event to the function

> _Note: this issue was identified against the code base at commit `f42419e01630977583bc09726b6f41b0d8951344`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: a subscription filter whose destinationArn is a same-account Lambda function never invokes that function for log events written to the subscribed group, so nothing the function would do with the delivered batch ever happens.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) — PutSubscriptionFilterRequest$destinationArn is documented as "The ARN of the destination to deliver matching log events to. Currently, the supported destinations are: ... A Lambda function belonging to the same account as the subscription filter, for same-account delivery." Recorded behavior: with an empty filterPattern (match everything) and a lambda:InvokeFunction permission whose Principal is logs.amazonaws.com and whose SourceArn is the log group ARN, each batch of log events written to the group is handed to the function asynchronously as the base64-encoded, gzip-compressed `awslogs.data` envelope containing `logEvents[].message`; the function runs, and anything it prints appears in its own /aws/lambda/<function-name> log group. The deliverable here is the effect: the destination function observes the event's message.
Emulator: the stack carrying the group, the role, the function, the AWS::Lambda::Permission and the AWS::Logs::SubscriptionFilter reaches CREATE_COMPLETE (the check's deploy helper raises otherwise, and it did not), CreateLogStream and PutLogEvents on the source group succeed, and then nothing arrives: FilterLogEvents on /aws/lambda/<destination function> with filterPattern "RECEIVED" returns an empty events list for the entire 120 s window — the group is either absent or empty. The subscribed function is never observed to have run. Hypothesis: the PutLogEvents ingest path does not fan matching events out to the group's subscription filters at all; if it does invoke, then the second possibility is that the invocation produces no log output — the related open issue 20261107T035448Z-cloudwatch-lambda-logs-a-synchronous-invoke-writes-no.md records that a Lambda invocation writes no events into its own log group, and that issue does not carry this check, so a fixer should confirm which of the two links is broken (and close both if both are).
Why: subscription filters are the standard real-time log pipeline — a client wires a group to a processing function and relies on every ingested batch reaching it. Against the emulator the filter is created and reported in the group's metadata while no event is ever delivered, so a log-processing pipeline silently does nothing and a test of it passes vacuously; the CloudFormation-deployed topology is indistinguishable from one with no filter at all.
Fix: the CloudWatch Logs ingest path (PutLogEvents) must evaluate each log group's subscription filters and, for a Lambda destinationArn, invoke the function asynchronously with the gzip+base64 `awslogs` payload (checking the logs.amazonaws.com invoke permission), and the Lambda invoke path must write the invocation's output into /aws/lambda/<function-name> so the delivery is observable.
Traces: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive.

## Evidence
Behavioral suite simparity-behavioral-aws-v1. Service heading: ### cloudwatch__lambda__logs (25/41 passed, 16 failed; this is the only one not marked ALREADY FILED / OWNED).

[mismatch] cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive, check line 127 -> helper line 152:
`AssertionError: destination Lambda log to contain delivered marker message not met within 120s. Last result: []`

What the check did, in order, all of it succeeding up to the poll:
1. create_stack with AWS::Logs::LogGroup (RetentionInDays 1), AWS::IAM::Role for lambda.amazonaws.com with AWSLambdaBasicExecutionRole, AWS::Lambda::Function (python3.12, inline handler that base64-decodes + gunzips event['awslogs']['data'] and prints 'RECEIVED: <message>' per logEvent), AWS::Lambda::Permission (Action lambda:InvokeFunction, Principal logs.amazonaws.com, SourceArn = the group ARN, SourceAccount = AWS::AccountId) and AWS::Logs::SubscriptionFilter (FilterPattern "", DestinationArn = the function ARN, DependsOn the permission). The stack reached CREATE_COMPLETE — the deploy helper asserts that explicitly.
2. logs.create_log_stream on the declared group, then logs.put_log_events with a single event whose message is a unique marker.
3. Poll logs.filter_log_events(logGroupName='/aws/lambda/<destination function>', startTime=<write time - 5 s>, filterPattern='RECEIVED') every 5 s for 120 s.

Expected: at least one event whose message contains the marker (the function ran and printed it). Observed: `[]` for the whole window — either ResourceNotFoundException on the function's log group (which the check maps to []) or an empty result.

Per-run values that are NOT requirements: the generated stack/group/stream/function names and the marker string. The requirement is that a log event written to a group carrying a Lambda-destination subscription filter reaches that function and that the function's output is observable in its own log group.

## Steps to Reproduce
1. Deploy (or create directly) a log group G, a Lambda function F (python3.12) whose handler base64-decodes and gunzips event['awslogs']['data'] and prints each logEvents[].message, a lambda:AddPermission on F with Principal 'logs.amazonaws.com' and SourceArn = G's ARN, and logs.put_subscription_filter(logGroupName=G, filterName='f', filterPattern='', destinationArn=<F's ARN>).
2. logs.create_log_stream(logGroupName=G, logStreamName='s'); logs.put_log_events(logGroupName=G, logStreamName='s', logEvents=[{'timestamp': <now ms>, 'message': 'MARKER-123'}]).
3. logs.filter_log_events(logGroupName='/aws/lambda/<F>', startTime=<now ms - 5000>, filterPattern='RECEIVED'). Expected: an event containing 'MARKER-123' within seconds. Observed: empty for 120 s.
4. Cross-check which link is broken: lambda.invoke(FunctionName=F, Payload=<a hand-built awslogs envelope>) and confirm (a) the function runs and (b) its output appears in /aws/lambda/<F>. If (a) works and (b) does not, the ingest fan-out and the log-write are both implicated.
5. Re-replay to confirm the fix: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive.
