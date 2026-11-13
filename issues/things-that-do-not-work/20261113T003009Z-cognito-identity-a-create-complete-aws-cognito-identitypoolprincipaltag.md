Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetPrincipalTagAttributeMap|ResourceNotFoundException
Issue-Commit: fea0c2e24ffe0fe10d2eaebdde89aa1a68933695

# cognito-identity: a CREATE_COMPLETE AWS::Cognito::IdentityPoolPrincipalTag writes no principal-tag attribute map, so GetPrincipalTagAttributeMap refuses the CFN-deployed mapping with ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `fea0c2e24ffe0fe10d2eaebdde89aa1a68933695`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPrincipalTagAttributeMap

## Description
REOPENS 20261023T185902Z-cloudformation-cognito-identity-sts-cloudformation-aws-cognito-identitypoolprincipaltag-is-rejected-as-an.md: its fix left the call refused. That issue's fix registered AWS::Cognito::IdentityPoolPrincipalTag so CreateStack is accepted and the stack now reaches CREATE_COMPLETE, but its stated requirement — "give it a provider that writes the principal-tag attribute map into cognito-identity state" / "the principal-tag mapping readable through cognito-identity GetPrincipalTagAttributeMap" — is not met: the read of the deployed mapping is still refused, now with a different code than the one that issue recorded (hence the replay key no longer matches it and the check is not reported as owned).

One cause (Cause 1 of 1): the CloudFormation resource provider for AWS::Cognito::IdentityPoolPrincipalTag reports success without writing the mapping into cognito-identity state.
Failing check ids for this cause: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected.

AWS: AWS::Cognito::IdentityPoolPrincipalTag provisions exactly the state SetPrincipalTagAttributeMap writes (Smithy, com.amazonaws.cognitoidentity#AWSCognitoIdentityService: SetPrincipalTagAttributeMap input IdentityPoolId @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, IdentityProviderName @required @length{1,128}, UseDefaults boolean, PrincipalTags map @length{0,50}). After the stack that declares it reaches CREATE_COMPLETE, GetPrincipalTagAttributeMap(IdentityPoolId=<the pool the resource names>, IdentityProviderName=<the provider the resource names>) is SERVED HTTP 200 and its output members echo that state: IdentityPoolId, IdentityProviderName, UseDefaults and PrincipalTags equal to the template's PrincipalTags map (in the failing check, UseDefaults=false and PrincipalTags={"tenant": "custom:tenant"} — the key/value pair is this template's value, the requirement is that whatever map the template declared is what is read back). GetPrincipalTagAttributeMap models ResourceNotFoundException (404) only for a pool/provider that genuinely carries no mapping; recorded behavior only for the exact not-found sentence AWS uses there.
Emulator: the stack reaches CREATE_COMPLETE (the check's _wait_stack passes and the Outputs resolve, so the pool id is now a well-formed REGION:GUID and no longer trips the identityPoolId pattern), yet the very next GetPrincipalTagAttributeMap on the mapped pool is refused with error code ResourceNotFoundException and message "No Principal Tags configured for Provider <identity provider name>" — fixed text "No Principal Tags configured for Provider ", variable part the IdentityProviderName taken from the request (replay: cognito-idp.us-west-2.amazonaws.com/us-west-2_29f841b73, a per-run value). The refusal is the correct answer for an UNMAPPED pool; the defect is that the CFN-declared mapping was never stored, so the pool is unmapped although CloudFormation reported the resource created.
Why: a client that deploys principal-tag mappings as infrastructure-as-code gets a success from CloudFormation and then a 404 from Cognito for the resource it just created. Everything that depends on the mapping is silently absent: the pool vends credentials without the mapped session tags, so a role whose trust policy grants only sts:AssumeRoleWithWebIdentity (no sts:TagSession) is NOT refused with InvalidIdentityPoolConfigurationException ("Invalid identity pool configuration. Check assigned IAM roles for this pool.") as AWS refuses it, and any aws:PrincipalTag/... condition written against those tags evaluates against no tags at all. An infrastructure-as-code tenant-isolation boundary deployed this way reads as deployed while enforcing nothing.
Fix: the CloudFormation resource provider for AWS::Cognito::IdentityPoolPrincipalTag — on CREATE and UPDATE it must apply IdentityPoolId / IdentityProviderName / PrincipalTags / UseDefaults into the cognito-identity principal-tag-attribute-map state that GetPrincipalTagAttributeMap reads (the same state SetPrincipalTagAttributeMap writes), remove it on DELETE, and only then report the resource CREATE_COMPLETE; the mapping must also be consumed by the credential-vending path so the pool requires sts:TagSession on the attached role.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected (check aborts at the first GetPrincipalTagAttributeMap, before the unmapped-pool control vend and the TagSession refusal/repair legs are reached).

Related but NOT covering: open issue 20261028T195228Z-cognito-identity-2-net-new-cloudformation-root-causes.md lists this check id, but both its causes are about the PHYSICAL IDS of the CFN resources (Ref of AWS::Cognito::IdentityPool resolving to the pool name; Ref of AWS::Cognito::IdentityPoolPrincipalTag not being "<poolId>|<providerName>"), and its Issue-Key records the old ValidationException on identityPoolId, which no longer reproduces — the pool id now resolves correctly and the call gets as far as a mapping lookup. The missing write of the mapping itself is a separate defect in the same provider.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (278/340 passed). Failing check cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected [error]:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetPrincipalTagAttributeMap operation: No Principal Tags configured for Provider cognito-idp.us-west-2.amazonaws.com/us-west-2_29f841b73

The call that was refused is the check's first read after the stack reached a terminal OK status:

  stored = ci.get_principal_tag_attribute_map(IdentityPoolId=mapped_pool, IdentityProviderName=provider)
  assert stored.get("PrincipalTags") == {"tenant": "custom:tenant"}

where mapped_pool comes from stack Output MappedPoolId={"Ref": "MappedPool"} and provider from Output ProviderName={"Fn::GetAtt": ["UserPool", "ProviderName"]}. The template's Resources declare:

  "PrincipalTag": {"Type": "AWS::Cognito::IdentityPoolPrincipalTag",
                   "Properties": {"IdentityPoolId": {"Ref": "MappedPool"},
                                  "IdentityProviderName": {"Fn::GetAtt": ["UserPool", "ProviderName"]},
                                  "PrincipalTags": {"tenant": "custom:tenant"},
                                  "UseDefaults": false}}

Expected: HTTP 200 with PrincipalTags == the declared map ({"tenant": "custom:tenant"} for this template), UseDefaults == false, and the echoed IdentityPoolId / IdentityProviderName. Actual: HTTP 404 ResourceNotFoundException "No Principal Tags configured for Provider <provider>" — i.e. no mapping exists for a resource CloudFormation reported as created. The pool id, provider name, user-pool id and stack suffix in the message are per-run values; the requirement is that the mapping declared in the template is readable at all.

Progress vs. the reopened issue: CreateStack is no longer refused with "Template format error: Unrecognized resource types: [AWS::Cognito::IdentityPoolPrincipalTag]", and the pool id Output is no longer the pool NAME (no ValidationException on 'identityPoolId' this run) — the remaining gap is purely that the provider writes nothing.

Model facts used (smithy_query, services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): GetPrincipalTagAttributeMap input IdentityPoolId @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, IdentityProviderName @required @length{1,128}; output IdentityPoolId, IdentityProviderName, UseDefaults, PrincipalTags @length{0,50}; errors InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500. SetPrincipalTagAttributeMap takes and returns the same four members.

The check carries the service token 'cognito-identity', which is stood up in this checkout (nothing is pending bootstrap).

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay suite simparity-behavioral-aws-v1; re-replay cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected to confirm a fix.
2. CreateStack with Capabilities=[CAPABILITY_NAMED_IAM] and a template declaring: an AWS::Cognito::UserPool (with a custom:tenant schema attribute) + AWS::Cognito::UserPoolClient; two AWS::Cognito::IdentityPool resources (MappedPool, UnmappedPool; AllowUnauthenticatedIdentities=false; CognitoIdentityProviders naming the user pool's ProviderName and the client); an AWS::Cognito::IdentityPoolPrincipalTag with IdentityPoolId={"Ref":"MappedPool"}, IdentityProviderName={"Fn::GetAtt":["UserPool","ProviderName"]}, PrincipalTags={"tenant":"custom:tenant"}, UseDefaults=false; and Outputs MappedPoolId={"Ref":"MappedPool"}, ProviderName={"Fn::GetAtt":["UserPool","ProviderName"]}.
3. Wait for CREATE_COMPLETE, read the Outputs, then call cognito-identity GetPrincipalTagAttributeMap(IdentityPoolId=<MappedPoolId>, IdentityProviderName=<ProviderName>).
   Expect HTTP 200 with PrincipalTags={"tenant":"custom:tenant"} and UseDefaults=false. Observe HTTP 404 ResourceNotFoundException "No Principal Tags configured for Provider <ProviderName>".
4. Downstream consequence to re-check after the fix: attach to MappedPool an authenticated role whose trust policy grants only sts:AssumeRoleWithWebIdentity (no sts:TagSession), GetId + GetCredentialsForIdentity with a user-pool ID token for a user carrying custom:tenant. Expect the mapped pool to refuse with InvalidIdentityPoolConfigurationException / "Invalid identity pool configuration. Check assigned IAM roles for this pool." while an identically-trusted UNMAPPED pool vends; then add sts:TagSession by an in-place UpdateStack and expect the same identity to vend through the mapped role.
