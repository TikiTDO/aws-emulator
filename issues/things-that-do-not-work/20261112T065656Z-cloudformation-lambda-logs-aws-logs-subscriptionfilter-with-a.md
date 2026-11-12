Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create
Issue-Key: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create|run#30|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Commit: dedd6f8a44979b422a25c34412c44daa0d34e6e0

# cloudformation__lambda__logs: AWS::Logs::SubscriptionFilter with a Lambda destination that has no invoke permission is created successfully instead of being refused and rolling the stack back (1 cause)

> _Note: this issue was identified against the code base at commit `dedd6f8a44979b422a25c34412c44daa0d34e6e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: CloudWatch Logs accepts a subscription filter whose Lambda destination has granted CloudWatch Logs no invoke permission, so the CloudFormation stack that declares it reaches CREATE_COMPLETE where AWS fails the resource and rolls the stack back.

AWS: Smithy: logs#PutSubscriptionFilter takes required logGroupName, filterName, filterPattern and destinationArn and models InvalidParameterException (client, HTTP 400) among its errors. AWS validates the destination before storing the filter: when destinationArn is a Lambda function ARN and that function's resource policy carries no lambda:InvokeFunction grant for Principal logs.amazonaws.com (and, on AWS, the matching SourceArn/SourceAccount), PutSubscriptionFilter is refused 400 InvalidParameterException with the fixed message `Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.` — that text has no variable parts. Through CloudFormation the resource create therefore fails and the stack settles ROLLBACK_COMPLETE, with the failing resource's ResourceStatusReason being the handler's wrapping of that service error: `Resource handler returned message: "Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function. (Service: CloudWatchLogs, Status Code: 400, Request ID: <requestId>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — here the service name, status code and HandlerErrorCode are fixed for this error class, while the request id and request token are per-run values.

Emulator: the stack with the ungranted AWS::Logs::SubscriptionFilter settles `CREATE_COMPLETE` and reports no failed-resource events at all. Replay detail: `AssertionError: a subscription filter with no invoke permission ended 'CREATE_COMPLETE': []` (the empty list is the set of stack events naming a failure). The filter is created, so the pre-store destination-permission check does not exist.

Why: a client relying on AWS's behaviour gets a stack that reports success for a pipeline that can never deliver. The missing-permission misconfiguration is exactly what AWS surfaces at deploy time; here it is swallowed, so neither the 400 InvalidParameterException nor the ROLLBACK_COMPLETE/ResourceStatusReason that a deploy pipeline keys off is ever produced, and the operator learns nothing until log events silently fail to arrive. Note: several open issues describe subscription filters not *delivering* to a Lambda destination (for example 20261029T180026Z-dynamodb-lambda-logs-cloudwatch-logs-subscription-filter-never-invokes-its-lambda.md and 20261110T153046Z-cloudwatch-lambda-logs-a-cloudformation-deployed-subscription-filter.md); none of them covers this create-time *refusal*, and none carries this check's replay key.

Fix: the logs PutSubscriptionFilter handler's validation — it must resolve a Lambda destinationArn and evaluate the function's resource policy for lambda:InvokeFunction by Principal logs.amazonaws.com before persisting the filter, refusing with InvalidParameterException and AWS's fixed message; and the CloudFormation AWS::Logs::SubscriptionFilter resource provider, so a refused create fails the resource (HandlerErrorCode InvalidRequest) and drives the stack to ROLLBACK_COMPLETE instead of CREATE_COMPLETE.

Traces: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create.

## Evidence
Failing check: [mismatch] cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create
Replay detail (verbatim, tail):
  File "check_cloudformation__lambda__logs_cfn_logs_subscription_filter_missing_permission_rejected_at_create.py", line 257, in run
    assert bad_status == "ROLLBACK_COMPLETE", (
  AssertionError: a subscription filter with no invoke permission ended 'CREATE_COMPLETE': []

Expected (AWS): PutSubscriptionFilter -> HTTP 400, error code InvalidParameterException, message `Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.`; the CloudFormation stack settles ROLLBACK_COMPLETE with the failing resource's ResourceStatusReason matching `Resource handler returned message: "<that message> (Service: CloudWatchLogs, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`.

Actual (emulator): StackStatus = CREATE_COMPLETE and no failure events ([]) — the subscription filter is stored despite the destination function granting logs.amazonaws.com nothing.

## Steps to Reproduce
1. Create a Lambda function (the subscription-filter destination) and do NOT add any resource policy statement for Principal logs.amazonaws.com.
2. Create a CloudWatch Logs log group.
3. CreateStack with a template declaring AWS::Logs::SubscriptionFilter whose LogGroupName is that group, FilterPattern is empty and DestinationArn is the function ARN (no AWS::Lambda::Permission for logs.amazonaws.com in the template).
4. Poll DescribeStacks until the stack status stops ending in _IN_PROGRESS.
Expected: ROLLBACK_COMPLETE, with a DescribeStackEvents entry for the filter carrying the wrapped InvalidParameterException reason above.
Observed: CREATE_COMPLETE with no failed-resource events; calling logs PutSubscriptionFilter directly with the same ungranted destination also succeeds instead of answering 400 InvalidParameterException.
