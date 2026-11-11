Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive
Issue-Key: apigateway__cloudformation__lambda|TestInvokeAuthorizer|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__cloudformation__lambda: CloudFormation-provisioned Lambda authorizer checks abort because apigateway TestInvokeAuthorizer answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
Cause 1 of 1: TestInvokeAuthorizer is a scaffold stub, so the only API-side way to exercise a CloudFormation-provisioned Lambda authorizer is unavailable.
AWS: Smithy TestInvokeAuthorizer (POST /restapis/{restApiId}/authorizers/{authorizerId}, success 200) returns TestInvokeAuthorizerResponse with clientStatus, log, latency, principalId, policy, authorization and claims; its modeled errors are BadRequestException (400), NotFoundException (404), TooManyRequestsException (429) and UnauthorizedException (401). Real AWS evaluates the authorizer and reports the decision in that payload — including the 403-style denial path the negative check expects and the stack-tag read the positive check expects.
Emulator: the call answers 501 with error code NotImplemented ('operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator'), so both checks abort before asserting anything about the authorizer.
Why: a client cannot test an authorizer at all; CI that validates a CFN-deployed authorizer's allow/deny decision and its missing-permission behaviour gets an unmodeled 501 rather than a decision payload or any of the four modeled errors.
Fix: the TestInvokeAuthorizer handler must evaluate the stored authorizer against the supplied headers/context and serialize TestInvokeAuthorizerResponse (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs).
Traces: apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative, apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive.

## Evidence
Expected: HTTP 200 with a TestInvokeAuthorizerResponse (clientStatus/policy/principalId), or one of the modeled 400/401/404/429 errors.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestInvokeAuthorizer operation: operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator (report lines 27162 and 27488).

## Steps to Reproduce
1. Deploy a CloudFormation stack with an AWS::ApiGateway::Authorizer of type TOKEN backed by an AWS::Lambda::Function.
2. Call apigateway test-invoke-authorizer --rest-api-id <id> --authorizer-id <id> --headers Authorization=<token>.
3. Expect HTTP 200 with the authorizer decision; observe HTTP 501 NotImplemented.
Re-replay apigateway__cloudformation__lambda.apigw-authorizer-lambda-cfn-permission-missing-negative and apigateway__cloudformation__lambda.apigw-authorizer-lambda-checks-cfn-stack-tag-positive.
