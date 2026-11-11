Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.cfn-apigateway-deployment-in-place-description-retained-api-move-then-not-found apigateway__cloudformation.cfn-apigateway-method-mock-in-place-update-retained-verb-replacement-then-not-found apigateway__cloudformation.cfn-apigateway-resource-retained-path-replacement-then-not-found apigateway__cloudformation.cfn-apigateway-stage-in-place-variables-retained-api-move-then-not-found
Issue-Key: apigateway__cloudformation.cfn-apigateway-deployment-in-place-description-retained-api-move-then-not-found|_expect#12|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudformation.cfn-apigateway-method-mock-in-place-update-retained-verb-replacement-then-not-found|_expect#15|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudformation.cfn-apigateway-resource-retained-path-replacement-then-not-found|_expect#15|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudformation.cfn-apigateway-stage-in-place-variables-retained-api-move-then-not-found|_expect#15|AssertionError|raise AssertionError(
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# In-place updates of AWS::ApiGateway::Deployment, Method, Resource and Stage are refused with an emulator-authored NotImplemented reason

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a CloudFormation stack update that changes only mutable properties of an API Gateway resource is refused instead of applied.

AWS: recorded behavior only (CloudFormation resource handlers are not in the Smithy model). An UpdateStack that changes a mutable property of AWS::ApiGateway::Deployment (Description), AWS::ApiGateway::Method (integration/mock settings), AWS::ApiGateway::Resource (PathPart on the same parent) or AWS::ApiGateway::Stage (Variables) is an in-place update: CloudFormation emits UPDATE_IN_PROGRESS then UPDATE_COMPLETE for the logical resource, the physical id is unchanged, and the stack reaches UPDATE_COMPLETE. API Gateway applies it through PatchOperations on UpdateDeployment / UpdateMethod / UpdateResource / UpdateStage.

Emulator: every one of those updates fails the resource and rolls the stack back. The stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [<LogicalId>]. ' and the resource event is UPDATE_FAILED with reason, verbatim: 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::<Type> in place; the resource was left as it is.' Fixed text of that reason is 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; the only variable part is the resource type name. No AWS error code or message has that shape — AWS never answers 'NotImplemented' for a supported property change.

Why: any client that updates a REST API through CloudFormation (console, CLI `aws cloudformation deploy`, CDK, SAM, Serverless) cannot change a description, a method integration, a resource path or stage variables at all; the stack rolls back and the caller sees a failure where AWS returns UPDATE_COMPLETE. Downstream checks that read the updated value, or that expect the prior physical id to be retained, never get to run.

Fix: the AWS::ApiGateway::* resource handlers' update path in the CloudFormation provisioner — they must classify a mutable-property change as an in-place update and call the API Gateway update operation, instead of returning the NotImplemented stub reason.

Traces: the four failing checks listed under trace_ids. The already-open issue 20261110T214424Z-apigateway-cloudformation-an-in-place-update-of-aws.md records the same refusal for AWS::ApiGateway::RestApi; it does not carry these four replay keys.

## Evidence
Expected: stack status UPDATE_COMPLETE; resource event UPDATE_IN_PROGRESS -> UPDATE_COMPLETE with the physical id unchanged.
Actual (cfn-apigateway-deployment-in-place-description-retained-api-move-then-not-found): AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Deployment]. '), expected 'UPDATE_COMPLETE'; failed events: [('Deployment', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::Deployment in place; the resource was left as it is.')]
Same reason with AWS::ApiGateway::Method ([Method]), AWS::ApiGateway::Resource ([Res]) and AWS::ApiGateway::Stage ([Stage]).

## Steps to Reproduce
1. CreateStack a template with an AWS::ApiGateway::RestApi plus a Deployment / Method / Resource / Stage; wait for CREATE_COMPLETE.
2. UpdateStack with only a mutable property changed (Deployment.Description, Method integration, Resource.PathPart, Stage.Variables).
3. DescribeStacks / DescribeStackEvents: the resource is UPDATE_FAILED with the NotImplemented reason and the stack is UPDATE_ROLLBACK_COMPLETE.
