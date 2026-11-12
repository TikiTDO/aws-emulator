Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback cognito-idp.deploy-identity-provider-cfn-federation-resolves cognito-idp.deploy-identity-provider-cfn-resolves-by-identifier cognito-idp.deploy-user-pool-cfn-enforces-configured-password-policy cognito-idp.deploy-user-pool-client-explicit-auth-flows-enforced cognito-idp.deploy-user-pool-password-policy-enforced cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope cognito-idp.refarch-cog-e2e-account-recovery-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|DescribeUserPool|InvalidParameterException
Issue-Key: cognito-identity-provider|GetIdentityProviderByIdentifier|InvalidParameterException
Issue-Key: cognito-identity-provider|DescribeUserPoolClient|InvalidParameterException
Issue-Key: cognito-idp.refarch-cog-e2e-account-recovery-lifecycle|phase#5|_PhaseError|raise _PhaseError(
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp: CloudFormation-provisioned user pools, app clients and identity providers cannot be addressed through the Cognito API (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPool

## Description
Two root causes that make every CloudFormation-deployed Cognito scenario dead on arrival. Both are grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 2 — the physical id of a CFN-created Cognito resource is a template/stack-derived name, not the id the Cognito service mints, so the owning service rejects it on its own id shape.
AWS: Smithy (cognito-idp) UserPoolType.Id and every operation's UserPoolId member are UserPoolIdType with length 1..55 and pattern ^[\w-]+_[0-9a-zA-Z]+$ (e.g. 'us-west-2_4cfda4cb3'); ClientIdType has pattern ^[\w+]+$. The physical resource id of AWS::Cognito::UserPool is that pool id, of AWS::Cognito::UserPoolClient that client id, and Ref/GetAtt and every cross-service call use them, so a stack output wired into DescribeUserPool / DescribeUserPoolClient / GetIdentityProviderByIdentifier / AdminCreateUser resolves and the call succeeds.
Emulator: the physical id is the template-derived name or '<stack>-<LogicalId>-<suffix>' ('sim2real-cog-upd-f5cadd769b93-pool', 'sim2real-cfn-delprot-0871a75d0333', 'sim2real-cog-stack-...-UserPoolClient-5BF917AAFF30'), and the Cognito operation then answers InvalidParameterException (400) '1 validation error detected: Value <id> at <userPoolId|clientId> failed to satisfy constraint: Member must satisfy regular expression pattern: ...'. The refusal is correct for the value; the value is what is wrong.
Why: nothing a stack creates can be reached through the Cognito API, so every deploy-then-exercise flow (password policy enforcement, auth-flow enforcement, deletion protection, identity-provider federation, M2M client-credentials tokens) stops at its first API call after the deploy.
Fix: the AWS::Cognito::UserPool / UserPoolClient / UserPoolIdentityProvider resource providers in the CloudFormation provisioner — create the resource through the owning service and store the id that service mints as PhysicalResourceId (what Ref returns). Open issue 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md records this as its Cause 8, but only for AdminCreateUser (Issue-Key cognito-identity-provider|AdminCreateUser|InvalidParameterException); it carries none of the replay keys below, which fail on DescribeUserPool, DescribeUserPoolClient and GetIdentityProviderByIdentifier, so a fixer could close it without re-running them.
Traces: cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement, cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback, cognito-idp.deploy-identity-provider-cfn-federation-resolves, cognito-idp.deploy-identity-provider-cfn-resolves-by-identifier, cognito-idp.deploy-user-pool-cfn-enforces-configured-password-policy, cognito-idp.deploy-user-pool-client-explicit-auth-flows-enforced, cognito-idp.deploy-user-pool-password-policy-enforced, cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope.

Cause 2 of 2 — a CFN-deployed Cognito pool's stack Outputs carry no OutputValue, so the account-recovery lifecycle cannot even start.
AWS: Smithy (cloudformation) Output declares OutputKey, OutputValue ('The value associated with the output'), Description and ExportName; a stack in CREATE_COMPLETE reports every declared Output with its resolved OutputValue.
Emulator: the Output entry exists with no OutputValue member, so the deploy phase raises KeyError: 'OutputValue' and the whole four-phase lifecycle aborts in phase 1 of 4.
Why: Outputs are the documented way a stack hands its pool id and client id to the caller; with no value, no CFN-deployed Cognito scenario can be driven at all.
Fix: Outputs resolution/serialization in the stack-describe path. This is the same root cause as Cause 1 of open issue 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md, which does not carry this check's replay key — it is recorded here so this check is re-replayed when that fix lands.
Traces: cognito-idp.refarch-cog-e2e-account-recovery-lifecycle.

## Evidence
Cause 1 (identical failure shape across eight checks):
- botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the DescribeUserPool operation: 1 validation error detected: Value 'sim2real-cog-upd-f5cadd769b93-pool' at 'userPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+_[0-9a-zA-Z]+$
- … when calling the DescribeUserPool operation: Value 'sim2real-cfn-delprot-0871a75d0333' at 'userPoolId' …
- … when calling the GetIdentityProviderByIdentifier operation: Value 'sim2real-cfn-idp-pool-91df14494f54' at 'userPoolId' …
- … when calling the DescribeUserPoolClient operation: 2 validation errors detected: Value 'sim2real-cog-authflows-d4cfa566dae4-pool' at 'userPoolId' … ; Value 'sim2real-cog-authflows-d4cfa566dae4-PoolClient-D6AD0C6E8A6D' at 'clientId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w+]+$
Expected: a pool id of the shape 'us-west-2_<alnum>' and a client id matching ^[\w+]+$ from Ref/GetAtt of the stack resource.
Cause 2: _PhaseError: [phase 1/4 deploy] KeyError: 'OutputValue' — DescribeStacks Outputs entries have no OutputValue member.

## Steps to Reproduce
Cause 1: CreateStack with a template declaring AWS::Cognito::UserPool (+ UserPoolClient, + UserPoolIdentityProvider), wait for CREATE_COMPLETE, take Ref of each resource (or the stack Output wired to it) and call DescribeUserPool / DescribeUserPoolClient / GetIdentityProviderByIdentifier with it -> expect the call to succeed; observe InvalidParameterException on the id pattern.
Cause 2: CreateStack with an Outputs section exporting the pool id, wait for CREATE_COMPLETE, DescribeStacks -> the Outputs entries lack OutputValue.
