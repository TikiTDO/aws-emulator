Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims
Issue-Key: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims|run#30|AssertionError|assert claims.get(ADDED_CLAIM) == ADDED_VALUE, (
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# cognito-idp__lambda: a PreTokenGeneration V2 trigger's claimsToAddOrOverride never reach the issued access token

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1: claims a PreTokenGeneration_V2_0 Lambda trigger asks to add to the access token are dropped, so the minted access token carries only the emulator's built-in claim set.
AWS: recorded behavior only (the token body and the trigger event/response are not Smithy-modelled; CreateUserPool models LambdaConfig.PreTokenGenerationConfig with LambdaVersion V2_0). A V2_0 PreTokenGeneration handler returns response.claimsAndScopeOverrideDetails.accessTokenGeneration with claimsToAddOrOverride, claimsToSuppress and scopesToAdd/scopesToSuppress; Cognito applies those to the access token it mints, so the decoded access-token payload contains each added/overridden claim with the handler's value, omits each suppressed claim, and its scope string reflects the scope edits.
Emulator: the added claim is absent from the access token. The decoded payload contains only ['auth_time', 'client_id', 'cognito:groups', 'exp', 'iat', 'iss', 'jti', 'origin_jti', 'scope', 'sub', 'token_use', 'username'] — i.e. the base claim set, with none of the trigger's accessTokenGeneration overrides applied.
Why: token enrichment is the whole point of the V2 trigger: applications put tenant ids, entitlements and custom scopes into the access token there and authorize on them downstream (API Gateway/AppSync/resource servers). With the overrides dropped, every authorization decision taken on an enriched claim fails, and suppression of a sensitive claim silently does not happen — the emulator hands out a token broader than the handler asked for.
Fix: the cognito-idp token-minting path's handling of the PreTokenGeneration V2 response — the claimsAndScopeOverrideDetails.accessTokenGeneration section (claimsToAddOrOverride, claimsToSuppress, scopesToAdd/scopesToSuppress) must be merged into the access-token claim set before signing. Related open issues that do NOT carry this check's replay key: 20261105T111951Z-cognito-idp-lambda-the-pretokengeneration-v2-trigger-does.md (triggerSource for refresh vs authentication) and 20261031T170200Z-cognito-idp-lambda-reopens-cognito-idp-lambda-preauthentication-pretokengeneration-triggers-every.md.
Traces: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims.

## Evidence
AssertionError: "the trigger's added claim did not reach the access token: ['auth_time', 'client_id', 'cognito:groups', 'exp', 'iat', 'iss', 'jti', 'origin_jti', 'scope', 'sub', 'token_use', 'username']". Expected: the decoded access-token payload contains the claim name the handler put in claimsAndScopeOverrideDetails.accessTokenGeneration.claimsToAddOrOverride with exactly the handler's value (the specific name/value are the check's own request values, not part of the requirement). Actual: that key is not present at all; only the base claim set is emitted.

## Steps to Reproduce
1) Create a Lambda function whose handler returns response.claimsAndScopeOverrideDetails.accessTokenGeneration = {"claimsToAddOrOverride": {"<custom claim>": "<value>"}, "claimsToSuppress": [...], "scopesToAdd": [...]} and grant cognito-idp.amazonaws.com permission to invoke it.
2) CreateUserPool with LambdaConfig.PreTokenGenerationConfig = {LambdaArn: <fn>, LambdaVersion: "V2_0"}; CreateUserPoolClient; create and confirm a user.
3) InitiateAuth (USER_PASSWORD_AUTH) and base64url-decode the returned AccessToken payload: the added claim is missing, suppressed claims are still present, and the scope string is unchanged.
