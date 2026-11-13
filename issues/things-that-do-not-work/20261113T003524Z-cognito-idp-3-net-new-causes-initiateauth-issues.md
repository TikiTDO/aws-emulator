Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement|_error#83|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback|_error#27|AssertionError|raise AssertionError(f"call was expected to fail but succeeded: {resp!r}")
Issue-Key: cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope|_error#18|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: fea0c2e24ffe0fe10d2eaebdde89aa1a68933695

# cognito-idp: 3 net-new causes — InitiateAuth issues tokens to a secret-bearing client without SECRET_HASH, CreateUserPoolClient accepts a code-flow client with no CallbackURLs, and GetClientToken issues a token for a scope the client was never granted

> _Note: this issue was identified against the code base at commit `fea0c2e24ffe0fe10d2eaebdde89aa1a68933695`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Three distinct gates that AWS enforces and the emulator does not. Each is numbered with its own failing check ids; all three must be closed.

Cause 1 of 3: InitiateAuth (USER_PASSWORD_AUTH) on an app client configured with a secret succeeds when AuthParameters carries no SECRET_HASH.
AWS: a confidential app client (GenerateSecret=true) must prove possession of the secret on every unauthenticated auth call. Without SECRET_HASH, InitiateAuth is refused with error code NotAuthorizedException (Smithy: InitiateAuth errors include NotAuthorizedException, httpError 403) and the message 'Client <clientId> is configured with secret but SECRET_HASH was not received' — the fixed text is 'Client ' ... ' is configured with secret but SECRET_HASH was not received' and the only variable part is the ClientId taken from the request. No AuthenticationResult is returned.
Emulator: the call returns HTTP 200 with a full AuthenticationResult (AccessToken, ExpiresIn 3600, TokenType Bearer, RefreshToken) for the user. The client in the check was created by CloudFormation with GenerateSecret=true, so either the secret was not persisted onto the replacement client or the SECRET_HASH precondition is not evaluated at all.
Why: a confidential client's secret is the only thing standing between a public endpoint and token issuance. Any caller that knows a username, a password and a client id gets tokens, and a client that correctly omits SECRET_HASH in a negative test is told it authenticated. Code written against the emulator ships without SECRET_HASH and is refused by AWS.
Fix: the cognito-idp InitiateAuth handler (.../src/ops/initiate_auth.rs) must check, before any password verification, whether the resolved app client has a secret and refuse as above when SECRET_HASH is absent (and verify it when present); and the app-client state written by the CloudFormation AWS::Cognito::UserPoolClient provider must carry the generated secret. Related open issue 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md lists SECRET_HASH among its causes and owns other checks; it does not own this one.
Traces: cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement.

Cause 2 of 3: CreateUserPoolClient accepts AllowedOAuthFlows ['code'] with AllowedOAuthFlowsUserPoolClient=true and no CallbackURLs, and creates the client.
AWS: the authorization-code and implicit flows have nowhere to redirect without a callback URL, so the request is refused with error code InvalidOAuthFlowException (Smithy: CreateUserPoolClient errors include InvalidOAuthFlowException, httpError 400) and the message 'CallbackUrls can not be empty when code flow or implicit flow is selected' — fixed text, no variable parts — and nothing is created (ListUserPoolClients must not show the client name).
Emulator: the call answers 200 and returns the created client (ClientId minted, AllowedOAuthFlows ['code'], AllowedOAuthScopes ['openid'], AllowedOAuthFlowsUserPoolClient true), so a client that can never complete a hosted-UI login exists.
Why: a misconfigured OAuth client is accepted at deploy time and only fails later at the redirect, where there is no error to attribute it to; the matching CloudFormation stack that relies on this refusal never rolls back. Negative-path tests of app-client configuration pass against the emulator and fail against AWS.
Fix: the cognito-idp CreateUserPoolClient handler (.../src/ops/create_user_pool_client.rs) validation — when AllowedOAuthFlowsUserPoolClient is true and AllowedOAuthFlows contains 'code' or 'implicit', require a non-empty CallbackURLs and otherwise refuse with the code and message above; UpdateUserPoolClient needs the same gate. Related open issue 20261024T114943Z-cognito-idp-user-pool-and-app-client-configuration.md owns cognito-idp.user-pool-client-code-flow-without-callback-urls-rejected but not this check.
Traces: cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback.

Cause 3 of 3: GetClientToken issues an access token for a scope that the resource server declares but that was never granted to the requesting app client.
AWS: the machine-to-machine token endpoint may only mint the scopes in the client's own AllowedOAuthScopes. A scope that exists on the resource server but is absent from the client's grant is refused with error code NotAuthorizedException (Smithy: GetClientToken errors include NotAuthorizedException, httpError 403, and InvalidParameterException; AWS uses NotAuthorizedException for this class, not InvalidParameterException) and the message 'Client is not allowed to request these scopes' — fixed text, no variable parts — and no token is returned.
Emulator: the call answers 200 with ClientAuthenticationResult { AccessToken, ExpiresIn 3600, TokenType 'Bearer' }, and the token's own 'scope' claim is the ungranted scope ('<resourceServerId>/write'). In the same check the wrong-secret boundary was refused correctly, so the authentication half works and only the scope-grant gate is missing.
Why: the client's AllowedOAuthScopes is the authorization boundary for M2M access; ignoring it lets any client holding its own secret mint tokens for every scope any resource server in the pool declares, and a resource server that trusts the token's scope claim grants access AWS would have denied.
Fix: the cognito-idp GetClientToken handler (.../src/ops/get_client_token.rs) must intersect the requested Scopes with the app client's AllowedOAuthScopes and refuse the request (not silently narrow it) when any requested scope is not granted.
Traces: cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope.

## Evidence
Cause 1 — SECRET_HASH not required:
  cognito-idp.deploy-cognito-idp-2-password-policy-schema-update-and-client-replacement, line 205:
    code, message = _error(lambda: cognito.initiate_auth(ClientId=new_client_id, AuthFlow="USER_PASSWORD_AUTH", AuthParameters={"USERNAME": early_user, "PASSWORD": _LOOSE_PASSWORD}))
    AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ChallengeParameters': {}, 'AuthenticationResult': {'AccessToken': 'eyJhbGciOiJSUzI1NiIs...', 'ExpiresIn': 3600, 'TokenType': 'Bearer', 'RefreshToken': '...'}}
  Expected (AWS): NotAuthorizedException, message 'Client <new client id> is configured with secret but SECRET_HASH was not received'; the client was deployed by CloudFormation with GenerateSecret=true.

Cause 2 — code flow without CallbackURLs accepted:
  cognito-idp.deploy-cognito-idp-3-deletion-protection-and-rollback, line 130:
    code, message = _error(lambda: _create_client_capturing(ctx, idp, ClientName=bad_name, UserPoolId=pool_id, GenerateSecret=False, AllowedOAuthFlowsUserPoolClient=True, AllowedOAuthFlows=["code"], AllowedOAuthScopes=["openid"], SupportedIdentityProviders=["COGNITO"]))
    AssertionError: call was expected to fail but succeeded: {'UserPoolId': 'us-west-2_<suffix>', 'ClientName': 'sim2real-oauth-nocb-<hex>', 'ClientId': '<minted>', 'AllowedOAuthFlows': ['code'], 'AllowedOAuthScopes': ['openid'], 'AllowedOAuthFlowsUserPoolClient': True, ...}
  Expected (AWS): InvalidOAuthFlowException, message 'CallbackUrls can not be empty when code flow or implicit flow is selected', and the client absent from ListUserPoolClients.

Cause 3 — ungranted scope accepted:
  cognito-idp.get-client-token-rejects-wrong-secret-and-undeclared-scope, line 94:
    code_undeclared_scope = _error(lambda: cognito.get_client_token(ClientId=client_id, Secret=correct_secret, Scopes=["<resourceServer>/write"]))
    AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ClientAuthenticationResult': {'AccessToken': 'eyJhbGciOiJSUzI1NiIs...', 'ExpiresIn': 3600, 'TokenType': 'Bearer'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  The issued token's payload carries "scope":"<resourceServer>/write" although the client's AllowedOAuthScopes is ['<resourceServer>/read'] (asserted earlier in the check and passing).
  Expected (AWS): NotAuthorizedException, message 'Client is not allowed to request these scopes'. The wrong-secret boundary in the same check was refused correctly with NotAuthorizedException 'Invalid client or secret'.

## Steps to Reproduce
Cause 1: create a user pool and an app client with GenerateSecret=true and ExplicitAuthFlows including ALLOW_USER_PASSWORD_AUTH; create and confirm a user with a permanent password; call InitiateAuth(ClientId=<that client>, AuthFlow='USER_PASSWORD_AUTH', AuthParameters={USERNAME, PASSWORD}) with no SECRET_HASH. The emulator returns 200 with AuthenticationResult; AWS refuses NotAuthorizedException 'Client <clientId> is configured with secret but SECRET_HASH was not received'.
Cause 2: call CreateUserPoolClient(UserPoolId=<pool>, ClientName=<name>, AllowedOAuthFlowsUserPoolClient=true, AllowedOAuthFlows=['code'], AllowedOAuthScopes=['openid'], SupportedIdentityProviders=['COGNITO']) with no CallbackURLs. The emulator creates the client; AWS refuses InvalidOAuthFlowException 'CallbackUrls can not be empty when code flow or implicit flow is selected'. Repeat with CallbackURLs=['https://example.com/callback'] as the positive control (accepted by both).
Cause 3: create a resource server with scopes 'read' and 'write'; create an app client with GenerateSecret=true, ExplicitAuthFlows=['ALLOW_CLIENT_TOKEN_AUTH'], AllowedOAuthFlows=['client_credentials'] and AllowedOAuthScopes=['<rs>/read'] only; call GetClientToken(ClientId, Secret=<correct>, Scopes=['<rs>/write']). The emulator returns 200 and a token whose scope claim is '<rs>/write'; AWS refuses NotAuthorizedException 'Client is not allowed to request these scopes'.
