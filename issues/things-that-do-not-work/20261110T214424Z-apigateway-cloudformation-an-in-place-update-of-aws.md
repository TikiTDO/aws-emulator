Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.cfn-apigateway-restapi-mock-in-place-update-retained-stage-replacement-then-not-found
Issue-Key: apigateway__cloudformation.cfn-apigateway-restapi-mock-in-place-update-retained-stage-replacement-then-not-found|_expect#22|AssertionError|raise AssertionError(
Issue-Commit: 8c5ca24b773ca2762f91097a2e2db632761ff691

# apigateway__cloudformation: an in-place update of AWS::ApiGateway::RestApi is refused with emulator prose (NotImplemented), so every template-managed REST API rolls the stack back on UpdateStack

> _Note: this issue was identified against the code base at commit `8c5ca24b773ca2762f91097a2e2db632761ff691`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the AWS::ApiGateway::RestApi resource handler has no update path at all — any UpdateStack that changes a mutable property of an existing REST API fails the resource and rolls the stack back.

AWS: CloudFormation updates AWS::ApiGateway::RestApi in place for its mutable properties (Name, Description, Body, BinaryMediaTypes, MinimumCompressionSize, ApiKeySourceType, DisableExecuteApiEndpoint, Policy, Tags — "Update requires: No interruption") by issuing API Gateway's update call. Smithy (apigateway): UpdateRestApi, PATCH /restapis/{restApiId}, input restApiId (httpLabel, required) + patchOperations (ListOfPatchOperation), output RestApi; modeled errors BadRequestException 400, ConflictException 409, NotFoundException 404, LimitExceededException 429, TooManyRequestsException 429, UnauthorizedException 401. The stack therefore settles UPDATE_COMPLETE, the API's physical id and RootResourceId are unchanged, the Method attached to it is untouched, and GetRestApi reports the new Description. 'NotImplemented' is not a CloudFormation resource-status reason nor a modeled API Gateway error: AWS never puts that word in a stack event.

Emulator: the UpdateStack that only changes the RestApi Description (and a MOCK integration response template) records event ('Api', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::RestApi in place; the resource was left as it is.') and the stack settles UPDATE_ROLLBACK_COMPLETE with status reason 'The following resource(s) failed to update: [Api]. '. The whole reason text is emulator-authored prose; only the resource logical id ('Api') comes from the request's template.

Why: a client cannot change a CloudFormation-managed REST API at all — no description, body, binary media type, policy or endpoint-configuration change can ever be applied, and because the stack rolls back, every later resource in the same change set is reverted too. Everything downstream of the update is unreachable as a result: the UpdateReplacePolicy Retain replacement of AWS::ApiGateway::Stage (new stage 'two' created, old stage 'one' retained) and the post-delete refusal of GetRestApi (NotFoundException, message 'Invalid API identifier specified {account}:{api_id}', where the sentence is fixed text and account id and api id are values from the request/state) are never exercised.

Fix: the update path of the AWS::ApiGateway::RestApi resource handler in the CloudFormation crate — it must diff the old and new properties and translate the mutable ones into UpdateRestApi patchOperations (replace /description, /name, /policy, /disableExecuteApiEndpoint, add/remove /binaryMediaTypes, …), reserving replacement for create-only property changes. No other open apigateway__cloudformation issue covers this; 20261109T123922Z-apigateway-cloudformation-6-net-new-causes-a-not.md is the nearest one and names six other causes (not-found wording, Ref values, validation reasons, request-model validation, gateway-response status code), none of them the missing RestApi update handler.

Traces: apigateway__cloudformation.cfn-apigateway-restapi-mock-in-place-update-retained-stage-replacement-then-not-found.

## Evidence
Expected: after CreateStack reaches CREATE_COMPLETE, an UpdateStack carrying a new RestApi Description and a new MOCK response template settles UPDATE_COMPLETE, keeps the same Api and Method physical ids, and GetRestApi reports the new description.

Actual: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Api]. '), expected 'UPDATE_COMPLETE'; failed events: [('Api', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::RestApi in place; the resource was left as it is.')]

## Steps to Reproduce
1. CreateStack with AWS::ApiGateway::RestApi (Name, Description 'first', EndpointConfiguration REGIONAL, DisableExecuteApiEndpoint true) + AWS::ApiGateway::Method (MOCK integration returning {"greeting": "hello"}) + AWS::ApiGateway::Deployment + AWS::ApiGateway::Stage (StageName 'one', UpdateReplacePolicy Retain); wait for CREATE_COMPLETE.
2. UpdateStack with the same template but Description 'second' and the mock body {"greeting": "bonjour"}.
3. Wait for the stack to settle: it reaches UPDATE_ROLLBACK_COMPLETE with the Api resource UPDATE_FAILED on the NotImplemented reason above, instead of UPDATE_COMPLETE.
4. Re-replay apigateway__cloudformation.cfn-apigateway-restapi-mock-in-place-update-retained-stage-replacement-then-not-found.
