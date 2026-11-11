Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive|_probe#8|Timeout|return resp.get("Messages", [])
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A CloudFormation custom resource Delete never completes, so the stack-delete check never reaches a verdict

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: the Delete lifecycle of a Lambda-backed `Custom::` resource never converges.
AWS: recorded behavior only - on DeleteStack, CloudFormation invokes the custom resource's ServiceToken function with RequestType `Delete`, waits for the SUCCESS response the function PUTs to the pre-signed ResponseURL, and then moves the resource and the stack to DELETE_COMPLETE within the normal stack-operation latency.
Emulator: the step never reaches a verdict - the stack-delete wait does not finish and the check ends `Check timed out after 60.0s`. No Delete invocation result or terminal stack status is ever observable.
Why: a client that models anything with a custom resource cannot tear the stack down - DeleteStack neither succeeds nor fails, so the stack is stuck and no error tells the client why. A hang is worse than a refusal because there is nothing to act on.
Fix: the custom-resource provider of the CloudFormation stack-delete path - the Delete-request dispatch to the ServiceToken function and the response-URL callback that must release the wait.

Traces: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive.

## Evidence
Expected: the Delete request reaches the custom resource's Lambda and the stack reaches DELETE_COMPLETE.
Actual: Error: Check timed out after 60.0s - no terminal stack status and no Delete invocation observed.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateStack with a Lambda function and a Custom:: resource whose ServiceToken is that function; wait for CREATE_COMPLETE.
3. DeleteStack and poll DescribeStacks / the function's invocation record - the Delete request never completes.
4. Re-replay apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive.
