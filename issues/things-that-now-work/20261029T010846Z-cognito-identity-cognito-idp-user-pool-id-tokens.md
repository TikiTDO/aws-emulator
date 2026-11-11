Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role
Issue-Key: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role|run#37|AssertionError|assert header.get("alg") == "RS256" and header.get("kid"), (
Issue-Commit: d05383a7a9fbb10dc2241138b0670096478c83a6

# cognito-identity__cognito-idp: user-pool ID tokens are signed with alg ES256 instead of RS256, so no Cognito token verifier accepts them (1 cause)

> _Note: this issue was identified against the code base at commit `d05383a7a9fbb10dc2241138b0670096478c83a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
One net-new root cause in the cognito-idp token-minting path, grounded in a failing behavioral check of this suite that the replay report marked neither ALREADY FILED nor OWNED. The check is a cognito-identity role-mapping scenario, but it never reaches the identity-pool assertions: it aborts on the JWS header of the ID token the user pool minted.

=== Cause 1 of 1: the ID token returned by AdminInitiateAuth is an ES256 JWS, not the RS256 JWS a Cognito user pool issues ===
AWS: Smithy (com.amazonaws.cognitoidentityprovider) — AdminInitiateAuth returns AuthenticationResultType.IdToken (TokenModelType, pattern ^[A-Za-z0-9-_=.]+$, sensitive, "Your user's ID token"). The model does not constrain the signing algorithm, so this is recorded behavior only: every Amazon Cognito user pool signs its ID and access tokens as a three-part compact JWS whose header is {"alg":"RS256","kid":"<key id>","typ":"JWT"}, with the matching RSA public key published in the pool's JWKS at https://cognito-idp.<region>.amazonaws.com/<user-pool-id>/.well-known/jwks.json (kty RSA, alg RS256). The recorded run asserts exactly that: the header's alg is the fixed literal RS256 and a kid is present; the kid VALUE is a per-run key id and is not part of the requirement.
Emulator: AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) returns an ID token whose header is {'alg': 'ES256', 'kid': '<key id>', 'typ': 'JWT'} — an ECDSA P-256 signature where AWS uses RSASSA-PKCS1-v1_5 with SHA-256. The kid is present; only the algorithm diverges.
Why: every published way of verifying a Cognito token pins RS256 — the AWS JWT verification guidance, aws-jwt-verify, and the common jose/PyJWT recipes pass algorithms=['RS256'] and fetch an RSA JWK by kid. A client that verifies the emulator's ID token the way it verifies AWS's rejects it outright (algorithm mismatch, or no RSA key for that kid), so no application or API-gateway/JWT-authorizer integration built against AWS can validate a token this emulator mints. Anything downstream that merely decodes the payload keeps working, which makes the break appear only at the security boundary — exactly where it matters.
Fix: the cognito-idp token-minting component shared by AdminInitiateAuth/InitiateAuth/AdminRespondToAuthChallenge (the signer that builds the ID/access-token JWS) and the pool key material it draws from: the pool's signing key must be an RSA key and the JWS header alg must be RS256, with the matching RSA JWK (kty=RSA, alg=RS256, same kid) exposed in the pool's JWKS document.
Traces: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role.
Related but NOT covering: open issue 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md reports the pool's JWKS document not being served at all; this cause is the signing algorithm of the token itself, it carries a different replay key, and the report marked this check neither OWNED nor ALREADY FILED. Note that fixing the algorithm and serving the JWKS are the two halves of making a minted token verifiable.

## Evidence
cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role (check_cognito_identity__cognito_idp_deep_cognito_identity_3_token_mapping_ambiguous_multi_role_claim_fallback_deny_and_custom_role.py:277):
  assert header.get("alg") == "RS256" and header.get("kid"), ...
  AssertionError: the ID token header must be an RS256 JWS with a kid, got {'alg': 'ES256', 'kid': 'IJmgsFVsEQVx5jtdZsN859EZwCYGAEoqjQ-gmi6mFeQ', 'typ': 'JWT'}

Expected (AWS): the base64url-decoded JWS header of AuthenticationResult.IdToken has alg == 'RS256' (fixed literal) and a non-empty kid (per-run value) matching an RSA JWK in the pool's JWKS.
Actual (emulator): alg == 'ES256'. The kid shown above is a per-run value and is not itself the defect.

The token is otherwise a well-formed three-part compact JWS (the check's structural decode assertion passed), and the failure occurs before any cognito-identity role-mapping assertion in the same check, so the rest of that scenario is untested behind this.

## Steps to Reproduce
1. cognito-idp: CreateUserPool, then CreateUserPoolClient(GenerateSecret=false, ExplicitAuthFlows=['ALLOW_ADMIN_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH']).
2. AdminCreateUser(MessageAction=SUPPRESS) + AdminSetUserPassword(Permanent=true).
3. AdminInitiateAuth(UserPoolId, ClientId, AuthFlow='ADMIN_USER_PASSWORD_AUTH', AuthParameters={USERNAME, PASSWORD}).
4. Take AuthenticationResult.IdToken, split on '.', base64url-decode the first segment and read the header: alg must be 'RS256' with a kid present. The emulator returns alg 'ES256'.
5. Cross-check: fetch https://cognito-idp.<region>.amazonaws.com/<user-pool-id>/.well-known/jwks.json and confirm a key with that kid, kty 'RSA', alg 'RS256', then verify the signature with algorithms=['RS256'] — verification must succeed as it does against AWS.
