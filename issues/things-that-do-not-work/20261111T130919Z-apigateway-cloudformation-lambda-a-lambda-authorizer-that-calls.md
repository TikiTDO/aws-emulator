Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative
Issue-Key: apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive|run#14|AssertionError|assert policy["Statement"][0]["Effect"] == "Allow", resp
Issue-Key: apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative|run#20|AssertionError|assert allowed_policy["Statement"][0]["Effect"] == "Allow", (
Issue-Commit: 768db93f7ee9a823eb9c7336f7c11f317ed4b425

# apigateway__cloudformation__lambda: a Lambda authorizer that calls cloudformation:DescribeStacks to read a stack's Tags always takes its failure branch, so the Allow decision is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `768db93f7ee9a823eb9c7336f7c11f317ed4b425`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new behavioral root cause, grounded in 2 failing checks of the simparity-behavioral-aws-v1 replay (apigateway__cloudformation__lambda 13/35). Every other failure in this service is marked OWNED by an open issue or ALREADY FILED and is NOT part of this issue. Related issues named for the fixer, neither of which carries these replay keys: 20261105T111951Z-apigateway-cloudformation-lambda-the-lambda-python-runtime-has.md (open; the python runtime shipped no boto3 at all — here `import boto3` plainly succeeds, because the handler runs and returns its own principalId, so this is a DIFFERENT defect) and 20261106T233903Z-lambda-rds-secretsmanager-the-in-process-lambda-runtime.md (marked fixed; the in-runtime SDK shim knew only sqs/events/eventbridge/dynamodb and was to be widened to every service the emulator serves — if the shim still has a bounded catalogue, `cloudformation` is outside it and that is this cause).

Cause 1 of 1: from inside an invoked Lambda function, boto3's cloudformation DescribeStacks does not yield the stack's Tags, so an authorizer that gates on a stack tag can only ever return Deny.
AWS: cloudformation DescribeStacks (Smithy com.amazonaws.cloudformation#DescribeStacks, awsQuery, output Stacks: list<Stack>) returns, for a stack named by StackName, a Stack whose Tags member carries the key/value pairs supplied to CreateStack — "A list of Tags that specify information about the stack" — and AWS's managed python runtime ships a complete boto3, so a handler doing boto3.client("cloudformation").describe_stacks(StackName=<name>) on a CREATE_COMPLETE stack created with Tags=[{Key:<k>,Value:<v>}] reads that pair back under Stacks[0].Tags. The authorizer under test takes its Allow branch exactly when that tag is present, and API Gateway then reports (TestInvokeAuthorizer) principalId and a policyDocument with Effect Allow and clientStatus 0.
Emulator: the authorizer function IS invoked and DOES return its own document (principalId "sim2real-user" and the method ARN of the simulated request appear in the response), but with Effect Deny — i.e. the handler took its `except`/fallback branch, which it reaches only when describe_stacks raises or when the tag is not in the returned Stack. The stack itself reached CREATE_COMPLETE and was created with Tags=[{"Key":"sim2real-access","Value":"granted"}] through the same emulator, so the tag exists in CloudFormation's own state. Hypothesis: either the in-runtime SDK shim cannot construct/dispatch a `cloudformation` client from inside an invoked function (the shape of the already-fixed rds-data defect, whose error document the handler's bare `except` swallows), or DescribeStacks' Stack view does not project the stored Tags. One of the two checks prints `describe_stacks failed: <type>: <message>` to the function's stdout, so the function's /aws/lambda log group distinguishes them.
Why: no Lambda function can make an authorization (or any other) decision from CloudFormation state. The positive check never observes an Allow, and the negative check cannot tell its intended refusal (the sibling role WITHOUT cloudformation:DescribeStacks) from the control (the role WITH the grant): both answer Deny, so an IAM-permission regression in the emulator or in a user's template is invisible. Any handler that reads stack outputs/tags to configure itself silently degrades to its error path instead of failing loudly.
Fix: whichever of the two components the function's log identifies — the in-process Lambda runtime's bundled SDK shim in services/service-com-amazonaws-lambda-awsgirapiservice (it must construct and dispatch a client for every service this emulator answers for, wired to the emulator endpoint with the execution role's credentials), or the Stack view CloudFormation's DescribeStacks projects (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stacks.rs, the stack_view projection used at the end of handle), which must carry the Tags recorded at CreateStack.
Traces: apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive, apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative.

Not filed: the clientStatus 403 that accompanies the Deny in both response dumps — that is the TestInvokeAuthorizer clientStatus defect, filed once against the apigateway service token in this same sweep; it is not a second cause here.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout. Both checks CreateStack (a single AWS::SNS::Topic) with Tags=[{"Key":"sim2real-access","Value":"granted"}], wait for CREATE_COMPLETE, create a python Lambda whose handler does `cfn = boto3.client("cloudformation"); resp = cfn.describe_stacks(StackName=event["authorizationToken"])` and returns Effect Allow only when the returned Stack's Tags contain sim2real-access=granted (Deny on any exception), wire it as a TOKEN authorizer, and call TestInvokeAuthorizer with the stack name as the token.

[mismatch] apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive
  AssertionError: assert policy["Statement"][0]["Effect"] == "Allow", resp
  resp = {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'clientStatus': 403, 'log': "... Starting authorizer: <id> of type TOKEN for the simulated request\n... Authorizer <fnAuthorizerName> returned a policy that does not allow execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<apiId>/test-invoke-stage/GET/: User is not authorized to access this resource with an explicit deny ...", 'principalId': 'sim2real-user', 'policy': '{"Version":"2012-10-17","Statement":[{"Action":"execute-api:Invoke","Effect":"Deny","Resource":"arn:aws:execute-api:<region>:<account>:<apiId>/test-invoke-stage/GET/"}]}'}

[mismatch] apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative
  AssertionError: expected Allow with cloudformation:DescribeStacks granted, got 'Deny' (full response: same shape as above, principalId 'sim2real-user', Effect Deny)

Expected (AWS): the control/positive leg returns a policyDocument with Effect Allow for the method ARN, because describe_stacks returned the stack with Tags containing sim2real-access=granted. The negative leg (an execution role WITHOUT cloudformation:DescribeStacks) returns Deny — so the two legs must differ.
Actual (emulator): both legs return Effect Deny with principalId 'sim2real-user' — the handler ran (its own principalId and the echoed methodArn prove it) and took its failure branch.
Stack name, api id, authorizer id, function name, account and region are per-run values; the requirement is that a tag written by CreateStack is readable through DescribeStacks from inside an invoked function.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. cloudformation create-stack --template-body '{"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{}}}}' --tags Key=sim2real-access,Value=granted ; wait for CREATE_COMPLETE.
3. Directly, to isolate the component: cloudformation describe-stacks --stack-name <that stack> from the host and confirm Stacks[0].Tags carries sim2real-access=granted.
4. CreateFunction (python3.x) whose index.py does `import boto3; print(boto3.client("cloudformation").describe_stacks(StackName=<that stack>))`, grant its role cloudformation:DescribeStacks, and lambda Invoke it. Observed: the handler's CloudFormation call does not produce the tagged Stack (the authorizer checks' fallback branch is taken); AWS returns the Stack with its Tags. Read the function's /aws/lambda log group for the `describe_stacks failed: <type>: <message>` line to tell an SDK-shim failure from a missing Tags projection.
5. Wire that function as a TOKEN authorizer (CreateAuthorizer + lambda AddPermission for apigateway.amazonaws.com) and call apigateway test-invoke-authorizer with the stack name as the Authorization header: observed policy Effect Deny; AWS: Effect Allow.
6. Re-replay the two check ids in trace_ids.
