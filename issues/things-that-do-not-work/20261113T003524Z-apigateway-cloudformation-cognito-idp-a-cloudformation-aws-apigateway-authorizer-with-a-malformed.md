Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__cognito-idp.deploy-apigateway-cognito-authorizer-invalid-provider-arn-rejected
Issue-Key: apigateway__cloudformation__cognito-idp.deploy-apigateway-cognito-authorizer-invalid-provider-arn-rejected|run#16|AssertionError|assert status == "CREATE_FAILED", f"rejected stack ended {status!r}"
Issue-Commit: fea0c2e24ffe0fe10d2eaebdde89aa1a68933695

# A CloudFormation AWS::ApiGateway::Authorizer with a malformed ProviderARNs entry reaches CREATE_COMPLETE instead of CREATE_FAILED

> _Note: this issue was identified against the code base at commit `fea0c2e24ffe0fe10d2eaebdde89aa1a68933695`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Cause 1 of 1: a COGNITO_USER_POOLS authorizer whose ProviderARNs entry is not a Cognito user-pool ARN is created instead of refused, so the stack succeeds.

AWS: API Gateway validates every entry of Authorizer$providerARNs for a COGNITO_USER_POOLS authorizer against real Cognito user pools. Smithy (apigateway): CreateAuthorizer input providerARNs -> ListOfARNs, errors include BadRequestException (httpError 400). CreateAuthorizer refuses the malformed entry with HTTP 400, and CloudFormation turns that into a resource failure: the Authorizer resource goes CREATE_FAILED, the stack ends CREATE_FAILED (the check creates the stack with DisableRollback=True), and the resource's ResourceStatusReason is the registry-handler envelope 'Resource handler returned message: "<relayed service message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)'. For this class the recorded reason's relayed message is the fixed text 'ProviderARNs need to be valid Cognito Userpools. Invalid ARNs-' followed by a newline and then the offending ARN value taken from the request, then the SDK envelope '(Service: ApiGateway, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)'; the recording's HandlerErrorCode is 'AlreadyExists'. Only the offending-ARN token, the request id and the attempt count are per-request values; the rest is fixed text. The API must then list no authorizer at all (GetAuthorizers items == []).

Emulator: the stack reaches CREATE_COMPLETE. The replay asserts status == 'CREATE_FAILED' and gets 'CREATE_COMPLETE', so the Authorizer resource was created with providerARNs = ['not-an-arn'] and no validation was performed. The positive control in the same check (the same template naming the stack's own pool via Fn::GetAtt UserPool.Arn) passes, so the resource type itself works; only the ARN validation gate is missing.

Why: a client deploying an authorizer with a typo'd or cross-account pool ARN is told the deployment succeeded. Against the emulator, every subsequent request through that authorizer is authorized (or silently unauthorized) by an authorizer bound to a pool that does not exist, and the template bug is never surfaced at deploy time — the exact failure CloudFormation exists to report. Negative-path tests of authorizer wiring pass against the emulator and fail against AWS.

Fix: the CreateAuthorizer handler's request validation in the apigateway service crate — when type == COGNITO_USER_POOLS, every providerARNs entry must parse as an ARN and resolve to an existing Cognito user pool, otherwise refuse 400 BadRequestException with the message above; and the CloudFormation provider for AWS::ApiGateway::Authorizer must propagate that refusal as a CREATE_FAILED resource with the relayed-message envelope rather than fabricating a successful create.

Traces: apigateway__cloudformation__cognito-idp.deploy-apigateway-cognito-authorizer-invalid-provider-arn-rejected.

## Evidence
Check apigateway__cloudformation__cognito-idp.deploy-apigateway-cognito-authorizer-invalid-provider-arn-rejected, verdict [mismatch]:

  File "check_apigateway__cloudformation__cognito_idp_deploy_apigateway_cognito_authorizer_invalid_provider_arn_rejected.py", line 245, in run
    assert status == "CREATE_FAILED", f"rejected stack ended {status!r}"
  AssertionError: rejected stack ended 'CREATE_COMPLETE'

Expected (AWS): stack CREATE_FAILED; the Authorizer resource's ResourceStatusReason relays 'ProviderARNs need to be valid Cognito Userpools. Invalid ARNs-\n<bad arn> (Service: ApiGateway, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' with HandlerErrorCode 'AlreadyExists'; GetAuthorizers on the stack's RestApi returns [].
Actual (emulator): stack CREATE_COMPLETE, the authorizer was created with providerARNs ['not-an-arn'], no CREATE_FAILED event and no relayed refusal.
Control in the same check (ProviderARNs = Fn::GetAtt UserPool.Arn) reached CREATE_COMPLETE and listed the authorizer correctly, so the divergence is isolated to the validation gate.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Cognito::UserPool, AWS::ApiGateway::RestApi (EndpointConfiguration Types REGIONAL) and AWS::ApiGateway::Authorizer { RestApiId: Ref RestApi, Type: COGNITO_USER_POOLS, ProviderARNs: ['not-an-arn'], IdentitySource: 'method.request.header.Authorization' }, with DisableRollback=True.
2. Poll DescribeStacks until the stack reaches a terminal status.
3. Observe CREATE_COMPLETE; AWS reports CREATE_FAILED with the Authorizer resource's relayed 400 from API Gateway.
4. Call GetAuthorizers on the stack's RestApi physical id: the emulator lists the authorizer; AWS lists none.
