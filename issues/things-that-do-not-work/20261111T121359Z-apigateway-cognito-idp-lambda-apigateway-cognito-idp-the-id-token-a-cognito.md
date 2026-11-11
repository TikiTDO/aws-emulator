Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-carry-custom-attributes-and-email-verified
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-carry-custom-attributes-and-email-verified|run#26|AssertionError|assert got == want, f"authorizer claims were {got!r}, expected {want!r}; all: {claims!r}"
Issue-Commit: c6eb6cb9b89be924e67ca1ea4b242861bd98eea1

# apigateway/cognito-idp: the ID token a Cognito user pool issues omits email_verified and every custom:* attribute, so a REST API COGNITO_USER_POOLS authorizer delivers requestContext.authorizer.claims without them

> _Note: this issue was identified against the code base at commit `c6eb6cb9b89be924e67ca1ea4b242861bd98eea1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
One net-new root cause.

Cause 1 of 1: the ID token minted by the user pool carries only a fixed core claim set, so the stored standard attribute `email_verified` and all developer-defined `custom:*` attributes never reach the token — and therefore never reach `requestContext.authorizer.claims` of a REST API method whose authorizationType is COGNITO_USER_POOLS.

AWS: a Cognito ID token's claim set is derived from the authenticating user's stored attribute set — every stored standard attribute and every developer-defined custom attribute appears as a claim of the same name, with the attribute's value rendered as a JSON string. Smithy models the attributes themselves: `AttributeType.Name` is an `AttributeNameType` (@length 1..32, @pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r ]+$`) and its documentation names `email` and `custom:department` as example values, so a `custom:`-prefixed attribute is a first-class stored attribute; `AttributeType.Value` is a string (@length 0..2048, @sensitive). The ID-token claim composition itself is not in the Smithy model — recorded behavior only, plus the API Gateway reference the check cites (https://docs.aws.amazon.com/apigateway/latest/developerguide/apigateway-integrate-with-cognito.html): a COGNITO_USER_POOLS authorizer copies the verified ID token's claims verbatim into `requestContext.authorizer.claims`, so a user created with `email_verified=true`, `custom:tier=<string>` and `custom:level=<number>` must surface claims `email_verified` = the stored value as a string (`"true"`), `custom:tier` = the stored string, and `custom:level` = the stored number rendered as a string (a Number-typed custom attribute is still a string claim).

Emulator: the authorizer delivers a claims map whose keys are exactly `aud, auth_time, cognito:username, email, exp, iat, iss, jti, origin_jti, sub, token_use`. `email_verified`, `custom:tier` and `custom:level` are absent (the check reads them as `None`) even though all three were supplied to AdminCreateUser and the pool Schema declares `tier` (String) and `level` (Number). Note what is NOT wrong here: the request is authorized (HTTP 200) and the passthrough does carry `email`, `cognito:username` and `sub`, so the API Gateway authorizer-to-Lambda plumbing works; the claim set the pool put in the ID token is what is short. The same missing-claim list is reported by the cognito-idp-only check `cognito-idp.id-token-claims-derive-from-the-user-attribute-set`, which is carried by the open issue 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md (its Cause 4 of 8). That open issue does not carry this check's replay key, so this cross-service occurrence is filed separately and should be closed together with it; fixing ID-token claim assembly should resolve both.

Why: a client relying on AWS's behavior breaks in two common ways. Authorization logic in the integration Lambda that keys off `custom:tier`/`custom:level`/`cognito:groups`-style claims sees `None` and either denies every caller or, worse, treats the missing claim as "no restriction" and admits callers it should refuse. Any code that gates on `email_verified == "true"` (the documented pattern for refusing unverified sign-ups) can never pass, so verified users are rejected. The attribute values are provably stored in the pool, so the application has no way to detect the loss other than the claim silently being absent.

Fix: ID-token claim assembly in the cognito-idp token builder — the claim set must be projected from the user's stored attribute set (standard attributes including `email_verified`, plus every `custom:`-prefixed attribute) with each value serialized as a JSON string, rather than from a hardcoded core claim list. The API Gateway COGNITO_USER_POOLS authorizer's claims passthrough needs no change: it already forwards whatever claims the verified token contains.

Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-carry-custom-attributes-and-email-verified.

## Evidence
Verbatim from the replay (check apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-carry-custom-attributes-and-email-verified, run#117):

AssertionError: authorizer claims were {'custom:tier': None, 'custom:level': None, 'email_verified': None}, expected {'custom:tier': 'sim2real-gold', 'custom:level': '42', 'email_verified': 'true'}; all: {'aud': '<clientId>', 'auth_time': '<epoch>', 'cognito:username': '<username>', 'email': '<username>@example.com', 'exp': '<epoch>', 'iat': '<epoch>', 'iss': 'https://<endpoint>/<poolId>', 'jti': '<uuid>', 'origin_jti': '<uuid>', 'sub': '<uuid>', 'token_use': 'id'}

Expected: the claims map contains `email_verified` whose value is the stored attribute value rendered as a string, `custom:tier` whose value is the stored string, and `custom:level` whose value is the stored Number rendered as a string. The fixed part of the requirement is the claim NAMES (`email_verified`, `custom:<name>` for every declared custom attribute) and that values are JSON strings; the concrete values above ('sim2real-gold', '42', 'true') come from this run's AdminCreateUser request and are not part of the requirement. The per-run values in the delivered map (client id, username, epochs, uuids, issuer host/port) are likewise not requirements.

Status/code: the request is correctly authorized — HTTP 200 from the stage, no error code involved. The divergence is purely the claim set.

## Steps to Reproduce
1. CreateUserPool with Schema=[{Name:'tier',AttributeDataType:'String',Mutable:true},{Name:'level',AttributeDataType:'Number',Mutable:true}].
2. CreateUserPoolClient(GenerateSecret=false, ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH']).
3. AdminCreateUser(MessageAction='SUPPRESS', UserAttributes=[email, email_verified='true', 'custom:tier'=<string>, 'custom:level'=<number-as-string>]) then AdminSetUserPassword(Permanent=true).
4. AdminInitiateAuth(AuthFlow='ADMIN_USER_PASSWORD_AUTH') and keep AuthenticationResult.IdToken. Base64-decode its payload: expect claims `email_verified`, `custom:tier`, `custom:level` to be present as strings — the emulator omits all three.
5. Create a REST API with a COGNITO_USER_POOLS authorizer (providerARNs=[poolArn], identitySource='method.request.header.Authorization'), a GET /secure method using it, an AWS_PROXY integration to a Lambda that echoes `event.requestContext.authorizer.claims`, add the apigateway.amazonaws.com invoke permission, and CreateDeployment(stageName='test').
6. GET <stageUrl>/secure with header `Authorization: <IdToken>` -> 200, but the echoed claims map lacks `email_verified`, `custom:tier` and `custom:level`.
