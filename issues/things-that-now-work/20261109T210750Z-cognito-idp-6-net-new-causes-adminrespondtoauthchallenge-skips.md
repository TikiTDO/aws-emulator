Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-respond-to-auth-challenge-requires-secret-hash-on-a-secret-client cognito-idp.admin-respond-to-auth-challenge-rejects-bad-responses-without-spending-the-session cognito-idp.change-password-validates-previous-before-policy-and-leaves-every-other-session-signed-in cognito-idp.confirm-forgot-password-refuses-before-any-code-exists-in-validation-order cognito-idp.openid-configuration-of-a-pool-without-a-domain-names-the-token-issuer-and-endpoints-under-it cognito-idp.openid-configuration-for-an-unknown-pool-under-the-issuer-rejected cognito-idp.access-token-verifies-rs256-under-an-issuer-jwks-key-distinct-from-the-id-tokens
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-requires-secret-hash-on-a-secret-client|expect_error#17|AssertionError|raise AssertionError(f"expected {code} but the call succeeded: keys {sorted(result)!r}")
Issue-Key: cognito-identity-provider|AdminRespondToAuthChallenge|InvalidPasswordException
Issue-Key: cognito-identity-provider|ChangePassword|InvalidParameterException
Issue-Key: cognito-identity-provider|ConfirmForgotPassword|NotAuthorizedException
Issue-Key: cognito-idp.openid-configuration-of-a-pool-without-a-domain-names-the-token-issuer-and-endpoints-under-it|_get_json#17|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.openid-configuration-for-an-unknown-pool-under-the-issuer-rejected|run#14|AssertionError|assert status == 200 and json.loads(body).get("issuer") == iss, (
Issue-Key: cognito-idp.access-token-verifies-rs256-under-an-issuer-jwks-key-distinct-from-the-id-tokens|run#23|AssertionError|assert shape == {"kty": "RSA", "alg": "RS256", "use": "sig"}, (
Issue-Commit: c41adfe5770fc8b3bf338ccef127c84b5f467887

# cognito-idp: 6 net-new causes — AdminRespondToAuthChallenge skips the app-client SECRET_HASH check, three refusal messages/classes are emulator-authored, the pool's OpenID discovery document is not served, and the access token is ES256

> _Note: this issue was identified against the code base at commit `c41adfe5770fc8b3bf338ccef127c84b5f467887`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge

## Description
Six net-new root causes in the cognito-idp user-pool data plane, all grounded in failing behavioral checks of this run. Cause 1 lets a confidential (secret-bearing) app client complete an auth challenge with no client secret at all, so the bundle is HIGH. A fixer must close all six before submitting.

Cause 1 of 6: AdminRespondToAuthChallenge does not enforce SECRET_HASH on an app client created with GenerateSecret=true.
AWS: Smithy AdminRespondToAuthChallenge takes ChallengeResponses (ChallengeResponsesType, sensitive map) and models NotAuthorizedException (httpError 403). When the named ClientId was created with a secret, AWS requires a SECRET_HASH entry in ChallengeResponses and refuses the call before applying the response. With SECRET_HASH absent the refusal is NotAuthorizedException with the message 'Client <clientId> is configured with secret but SECRET_HASH was not received' — fixed text 'Client ' / ' is configured with secret but SECRET_HASH was not received', variable part: the ClientId from the request. With a SECRET_HASH that does not match base64(HMAC-SHA256(key=ClientSecret, msg=Username+ClientId)) the refusal is NotAuthorizedException 'Unable to verify secret hash for client <clientId>' — fixed text 'Unable to verify secret hash for client ', variable part: the ClientId from the request. Neither refusal may apply the NEW_PASSWORD, so the user stays FORCE_CHANGE_PASSWORD.
Emulator: the call with no SECRET_HASH SUCCEEDS with HTTP 200 and returns a full AuthenticationResult (response keys ['AuthenticationResult','ChallengeParameters','ResponseMetadata']) — the secret is never consulted.
Why: a caller that knows only a ClientId and a username can finish a NEW_PASSWORD_REQUIRED challenge and be issued access/ID/refresh tokens for a confidential client whose whole point is that the secret is required; the client secret is not a credential in the emulator.
Fix: the SECRET_HASH verification in the AdminRespondToAuthChallenge handler (it must run before the challenge response is applied), the same gate AdminInitiateAuth already applies.
Traces: cognito-idp.admin-respond-to-auth-challenge-requires-secret-hash-on-a-secret-client.

Cause 2 of 6: the weak-password refusal of a NEW_PASSWORD_REQUIRED challenge response uses the ChangePassword wording.
AWS: Smithy AdminRespondToAuthChallenge models InvalidPasswordException (httpError 400). When the NEW_PASSWORD in ChallengeResponses violates the pool's password policy AWS answers InvalidPasswordException with the message 'Password does not conform to policy: <first violated rule>' — fixed text 'Password does not conform to policy: ', variable part: the violated policy rule as AWS names it ('Password not long enough' for a password below MinimumLength). AWS uses the other phrasing ('Password did not conform with policy: ...') on ChangePassword, not here; the same check asserts that ChangePassword wording separately and it passes.
Emulator: InvalidPasswordException with 'Password did not conform with policy: Password not long enough' — the ChangePassword phrasing on the challenge path.
Why: clients and test suites that match on the documented challenge-path message do not recognise the refusal, and the two code paths are indistinguishable.
Fix: the InvalidPasswordException message built by the NEW_PASSWORD_REQUIRED branch of the AdminRespondToAuthChallenge handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_respond_to_auth_challenge.rs).
Traces: cognito-idp.admin-respond-to-auth-challenge-rejects-bad-responses-without-spending-the-session.

Cause 3 of 6: ChangePassword's missing-parameter refusal names the wire member instead of the parameter AWS names.
AWS: Smithy ChangePassword input has PreviousPassword (optional) and ProposedPassword/AccessToken (required), and models InvalidParameterException (httpError 400). A ChangePassword call that omits PreviousPassword is refused with InvalidParameterException and the message 'Missing required parameter OldPassword' — the whole string is fixed text; AWS names the parameter OldPassword in this message, not the wire member name.
Emulator: InvalidParameterException 'Missing required parameter PreviousPassword'.
Why: a client matching on AWS's message text treats the refusal as an unknown error class.
Fix: the missing-parameter validation message in the ChangePassword handler.
Traces: cognito-idp.change-password-validates-previous-before-policy-and-leaves-every-other-session-signed-in.

Cause 4 of 6: ConfirmForgotPassword answers the wrong error class when no reset code is outstanding for an existing user.
AWS: Smithy ConfirmForgotPassword models ExpiredCodeException, CodeMismatchException, InvalidPasswordException, NotAuthorizedException (403) and UserNotFoundException (404). Its validation order is: the app client's secret hash, then the user, then the code, ahead of the password policy. For a user that exists in the pool and has no outstanding reset code, any ConfirmationCode is refused with ExpiredCodeException and the message 'Invalid code provided, please request a code again.' (entirely fixed text) — including when the proposed Password would also violate the policy. UserNotFoundException 'Username/client id combination not found.' is reserved for a username no user carries under a client with PreventUserExistenceErrors=LEGACY; under PreventUserExistenceErrors=ENABLED an unknown username is masked as the same ExpiredCodeException.
Emulator: NotAuthorizedException 'Username/client id combination not found.' for an existing, CONFIRMED user — the wrong error code (NotAuthorizedException instead of ExpiredCodeException), the wrong message, and the user-existence message leaked for a user that does exist.
Why: a reset flow cannot distinguish 'request a new code' from 'these credentials are not authorized', and the 403/400 split means retry logic keyed on the status also diverges.
Fix: the validation order and error selection in the ConfirmForgotPassword handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/confirm_forgot_password.rs).
Traces: cognito-idp.confirm-forgot-password-refuses-before-any-code-exists-in-validation-order.

Cause 5 of 6: the user pool's OpenID Connect discovery document is not served under the token issuer.
AWS: recorded behavior only (this is the hosted OIDC endpoint, not a Smithy operation). GET <iss>/.well-known/openid-configuration, where <iss> is the iss claim of the pool's tokens (…/<poolId>), answers HTTP 200 with Content-Type application/json and a document whose issuer equals <iss> and whose endpoints all sit under that issuer: jwks_uri=<iss>/.well-known/jwks.json, authorization_endpoint=<iss>/authorize, token_endpoint=<iss>/token, userinfo_endpoint=<iss>/userInfo, revocation_endpoint=<iss>/revoke, end_session_endpoint=<iss>/logout, plus id_token_signing_alg_values_supported=['RS256'], response_types_supported=['code','token'], subject_types_supported=['public'], scopes_supported=['openid','email','phone','profile'], token_endpoint_auth_methods_supported=['client_secret_basic','client_secret_post']. The only variable part of every value is <iss> itself. For a well-formed pool id in the region that no pool carries, both that path and /.well-known/jwks.json answer HTTP 404 with the JSON body {"message": "User pool <poolId> does not exist."} — fixed text 'User pool ' / ' does not exist.', variable part: the pool id from the request path.
Emulator: both paths answer HTTP 404 with {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the request never reaches a cognito-idp route at all, so the positive document is missing and the nonexistent-pool 404 carries the generic router error instead of naming the pool. Note that <iss>/.well-known/jwks.json DOES resolve in the same run (cause 6 below reads keys from it), so this is the discovery-document route specifically, plus the unknown-pool branch of the well-known routing.
Why: every OIDC client, and every relying party that bootstraps from discovery (API Gateway JWT authorizers, aws-jwt-verify, generic OIDC libraries), cannot find the pool's endpoints or its signing keys, and a 404 that says 'UnknownService' is indistinguishable from the endpoint being wrong.
Fix: the pool's well-known endpoint routing and the discovery-document serializer (the same data-plane route family that already serves /<poolId>/.well-known/jwks.json), including its unknown-pool 404 body.
Traces: cognito-idp.openid-configuration-of-a-pool-without-a-domain-names-the-token-issuer-and-endpoints-under-it, cognito-idp.openid-configuration-for-an-unknown-pool-under-the-issuer-rejected.

Cause 6 of 6: the access token is signed with ES256 under an EC JWKS key, and not under a key distinct from the ID token's.
AWS: recorded behavior only for the key material (the signing algorithm is asserted by the OIDC discovery document, which declares id_token_signing_alg_values_supported=['RS256']). A user pool signs both its ID token and its access token with RS256; the kid in each token's JWS header resolves in <iss>/.well-known/jwks.json to a JWK of shape {kty:'RSA', alg:'RS256', use:'sig'} carrying the RSA modulus n and exponent e (RFC 7518 §6.3.1), the signature verifies as RSASSA-PKCS1-v1_5 with SHA-256 over the signing input, and the access token's kid is a DIFFERENT key of that JWKS from the ID token's kid — a pool keeps separate key pairs for the two token types.
Emulator: the JWKS key the access token's kid names is {kty:'EC', alg:'ES256', use:'sig'}, so the token is ES256-signed with no n/e to verify against, and the distinct-key property cannot hold.
Why: any verifier configured for RS256 — which is what the pool's own discovery document advertises — rejects every access token the emulator issues, and a verifier that pins the access-token key separately from the ID-token key cannot be configured at all.
Why this is not already covered: the open issue 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md records the ES256 signing of the ID token (its cause 1) and the missing JWKS document (its cause 2) under the check cognito-idp.cognito-jwt-claims-verifiable; it does not carry the access-token key's replay key, nor the requirement that the access token name a distinct RSA key of the same JWKS. The replay did not mark this check as owned by it. A fixer closing that issue should re-replay this check too.
Fix: the pool's token-signing key material and the token signer (both token types must use RS256 under separate RSA key pairs published in the pool's JWKS).
Traces: cognito-idp.access-token-verifies-rs256-under-an-issuer-jwks-key-distinct-from-the-id-tokens.

## Evidence
Verbatim from the replay output of this run:

Cause 1 — cognito-idp.admin-respond-to-auth-challenge-requires-secret-hash-on-a-secret-client:
  AssertionError: expected NotAuthorizedException but the call succeeded: keys ['AuthenticationResult', 'ChallengeParameters', 'ResponseMetadata']
  (expected: NotAuthorizedException 'Client <clientId> is configured with secret but SECRET_HASH was not received')

Cause 2 — cognito-idp.admin-respond-to-auth-challenge-rejects-bad-responses-without-spending-the-session:
  AssertionError: expected ('InvalidPasswordException', 'Password does not conform to policy: Password not long enough'), got ('InvalidPasswordException', 'Password did not conform with policy: Password not long enough')

Cause 3 — cognito-idp.change-password-validates-previous-before-policy-and-leaves-every-other-session-signed-in:
  AssertionError: expected ('InvalidParameterException', 'Missing required parameter OldPassword'), got ('InvalidParameterException', 'Missing required parameter PreviousPassword')

Cause 4 — cognito-idp.confirm-forgot-password-refuses-before-any-code-exists-in-validation-order:
  AssertionError: expected ('ExpiredCodeException', 'Invalid code provided, please request a code again.'), got ('NotAuthorizedException', 'Username/client id combination not found.')

Cause 5 — cognito-idp.openid-configuration-of-a-pool-without-a-domain-names-the-token-issuer-and-endpoints-under-it:
  AssertionError: DATAPLANE_UNIMPLEMENTED: <iss>/.well-known/openid-configuration answered HTTP 404: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  and cognito-idp.openid-configuration-for-an-unknown-pool-under-the-issuer-rejected:
  AssertionError: expected the pool's discovery document to serve its issuer, got 404 '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  (that check then expects, for a well-formed pool id no pool carries, 404 with body {"message": "User pool <poolId> does not exist."} on both /.well-known/openid-configuration and /.well-known/jwks.json)

Cause 6 — cognito-idp.access-token-verifies-rs256-under-an-issuer-jwks-key-distinct-from-the-id-tokens:
  AssertionError: expected the access token's key to be an RSA RS256 signing key, got {'kty': 'EC', 'alg': 'ES256', 'use': 'sig'}

## Steps to Reproduce
All steps use a fresh user pool created with CreateUserPool and an app client from CreateUserPoolClient.

Cause 1: CreateUserPoolClient(GenerateSecret=true, ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH']); AdminCreateUser with a TemporaryPassword and MessageAction=SUPPRESS; AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) with the correct SECRET_HASH to obtain a NEW_PASSWORD_REQUIRED Session; then AdminRespondToAuthChallenge(ChallengeName='NEW_PASSWORD_REQUIRED', Session=<that session>, ChallengeResponses={'USERNAME':<user>,'NEW_PASSWORD':<policy-conformant password>}) with NO SECRET_HASH -> expect NotAuthorizedException 'Client <clientId> is configured with secret but SECRET_HASH was not received'; repeat with a wrong SECRET_HASH -> expect NotAuthorizedException 'Unable to verify secret hash for client <clientId>'; AdminGetUser -> UserStatus must still be FORCE_CHANGE_PASSWORD.

Cause 2: on a live NEW_PASSWORD_REQUIRED session, AdminRespondToAuthChallenge with ChallengeResponses={'USERNAME':<user>,'NEW_PASSWORD':'Ab1'} -> expect InvalidPasswordException 'Password does not conform to policy: Password not long enough'.

Cause 3: sign a user in, then ChangePassword(AccessToken=<token>, ProposedPassword=<new>) with PreviousPassword omitted -> expect InvalidParameterException 'Missing required parameter OldPassword'.

Cause 4: AdminCreateUser a user with no verified email or phone (so no reset code can exist) under a client with PreventUserExistenceErrors='LEGACY'; call ConfirmForgotPassword(ClientId=<plain client>, Username=<that user>, ConfirmationCode='123456', Password=<policy-conformant password>) -> expect ExpiredCodeException 'Invalid code provided, please request a code again.'

Cause 5: AdminInitiateAuth to obtain an ID token, read its iss claim, then unsigned GET <iss>/.well-known/openid-configuration -> expect 200 application/json with issuer=<iss> and jwks_uri/authorization_endpoint/token_endpoint/userinfo_endpoint/revocation_endpoint/end_session_endpoint all under <iss>. Then GET the same path with the pool id replaced by a well-formed pool id no pool carries -> expect 404 with body {"message": "User pool <poolId> does not exist."}, and the same for /.well-known/jwks.json.

Cause 6: AdminInitiateAuth; base64url-decode the AccessToken's JWS header; GET <iss>/.well-known/jwks.json; look up the header's kid -> expect a JWK {kty:'RSA', alg:'RS256', use:'sig'} with n and e, a header alg of 'RS256', an RSASSA-PKCS1-v1_5/SHA-256 signature that verifies under it, and a kid different from the ID token's kid (which must also be in the same JWKS).
