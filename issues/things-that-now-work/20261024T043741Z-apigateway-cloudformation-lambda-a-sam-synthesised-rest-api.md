Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https
Issue-Key: apigateway__cloudformation__lambda|GetStages|NotFoundException
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__cloudformation__lambda: a SAM-synthesised REST API is unaddressable through the API Gateway API — GetStages answers NotFoundException for the stack's ServerlessRestApi, whose id is the resource name, not a REST API id (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStages

## Description
Cause 1 of 1: the physical id CloudFormation gives an AWS::Serverless::Api / AWS::ApiGateway::RestApi is not an API Gateway REST API id, so no API Gateway call can address the deployed API.
AWS: Smithy: GetStages (GET /restapis/{restApiId}/stages) takes restApiId as a required httpLabel and returns Stages{item: ListOfStage}; NotFoundException 404 is modeled for an unknown api. The restApiId a client reads out of a deployed stack (the RestApi resource's PhysicalResourceId / a Ref of it) is the API's 10-character generated id, so GetStages on it returns the implicitly-created stage and the API then serves HTTPS at its invoke URL.
Emulator: GetStages answers HTTP 404 NotFoundException with the message 'Invalid API identifier specified 000000000000:sim2real-xsvc-stk-<hash>-ServerlessRestApi-<suffix>' — i.e. the id handed back for the stack's ServerlessRestApi resource is an account-qualified RESOURCE NAME, not a REST API id, and no such API exists in the API Gateway store. Every subsequent leg (stage lookup, deployment id shape, invoking the synthesised endpoint) is unreachable.
Why: a client that deploys an API from a SAM/CloudFormation template and then drives it through the API Gateway SDK — the normal deploy-then-verify flow — cannot resolve its own API at all. The 404 is indistinguishable from 'the stack did not create an API'.
Fix: the CloudFormation resource provisioner for AWS::ApiGateway::RestApi / the AWS::Serverless-2016-10-31 transform output — the physical resource id (and any Ref/GetAtt of it) must be the id minted by the API Gateway CreateRestApi state, so GetStages/GetRestApi resolve it. Open issue 20261023T125454Z-apigateway-cloudformation-provisioned-api-gateway-resources-are-unaddressable-through.md describes this family but does not carry this check key.
Traces: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https.

## Evidence
botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetStages operation: Invalid API identifier specified 000000000000:sim2real-xsvc-stk-54c0d63bd72f-ServerlessRestApi-5E20D110A503. Expected: a 200 Stages response for the api id the stack reports for its ServerlessRestApi resource.

## Steps to Reproduce
1. create_stack with a SAM template (Transform: AWS::Serverless-2016-10-31, CAPABILITY_AUTO_EXPAND) containing a Serverless::Function with an Api event, so SAM synthesises the implicit ServerlessRestApi.
2. Wait for CREATE_COMPLETE, read the RestApi's physical resource id (describe_stack_resources) or a Ref output of it.
3. apigateway.get_stages(restApiId=<that id>) -> observe 404 NotFoundException naming '000000000000:<stack>-ServerlessRestApi-<suffix>' instead of the stack's stages.
4. Or replay apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https.
