Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role
Issue-Key: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role|run#42|AssertionError|assert (iss_parts is not None and iss_parts.scheme == "https"
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# cognito-identity__cognito-idp: the ID token's iss claim is the bare emulator listener (https://<host>:<port>/<poolId>), so the cross-service Cognito Identity token-mapping scenario never reaches the role-mapping arms

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: the `iss` claim of an ID token minted by (Admin)InitiateAuth names the socket the emulator was reached on instead of a Cognito user-pool issuer, which aborts the cross-service identity-pool token-mapping scenario before any Cognito Identity role-mapping behavior is exercised.

AWS: recorded behavior only — the JWT body is not Smithy-modelled (Smithy: AdminInitiateAuth returns AuthenticationResultType.IdToken, an opaque string; errors NotAuthorizedException/403 etc. are irrelevant here, the call succeeds). An ID token AWS mints for user pool <poolId> in region <region> carries iss = https://cognito-idp.<region>.<domain>/<poolId>. The requirement is the SHAPE, not one recording's values: scheme is https; the host's first two labels are the fixed literal `cognito-idp` followed by the pool's region; at least one further non-empty label follows (the deployment's domain); the path is exactly `/<poolId>` with the pool id taken from the request/state; there is no query and no fragment. The `<domain>` itself and any port are environment values and are NOT part of the requirement.

Emulator: iss is the raw listener address with no cognito-idp host and no region label — the replay shows `iss='https://127.0.0.1:41617/us-west-2_695cb073a'` (host/port are per-run; the defect is that the first labels are not `cognito-idp`.`<region>`). The pool id path segment is correct.

Why: a client validating a Cognito token derives the JWKS/discovery URL and performs the issuer check from `iss`. An issuer that identifies only the listener fails issuer validation in every standard verifier, cannot be distinguished from another provider's or another region's issuer, and makes the Cognito Identity side of the boundary unreachable: this check reads the ambiguity off the target's own ID token before predicting any vend outcome, so the malformed issuer stops it before the Token/AuthenticatedRole fallback arm, the Token/Deny refusal (NotAuthorizedException, message `The ambiguous role mapping rules for: <provider> denied this request.`), and the CustomRoleArn in/out-of-claim arms (NotAuthorizedException, message `CustomRoleARN: <roleArn> is not present in the token in the cognito:roles claim`) are ever evaluated. Those Cognito Identity behaviors therefore have no verdict at all.

Fix: the cognito-idp per-pool issuer computation shared by the token-minting path and the OpenID discovery/JWKS documents — it must render `https://cognito-idp.<region>.<domain>/<poolId>` from the pool's region and id rather than echoing the inbound Host header. Related open issue that does NOT cover this check (it is grounded in cognito-idp-only check ids and this failure was not attributed to it by the replay): 20261109T124858Z-cognito-idp-5-net-new-causes-tokens-are.md, Cause 2 of 5.

Traces: cognito-identity__cognito-idp.deep-cognito-identity-3-token-mapping-ambiguous-multi-role-claim-fallback-deny-and-custom-role.

## Evidence
Expected: the ID token returned by AdminInitiateAuth (AuthFlow=ADMIN_USER_PASSWORD_AUTH) carries iss = https://cognito-idp.<region>.<domain>/<userPoolId> — fixed text `https://cognito-idp.`, then the pool's region, then the deployment domain, then path `/<userPoolId>`.

Actual (replay): AssertionError: the ID token must be issued by https://cognito-idp.us-west-2.<domain>/us-west-2_695cb073a, got iss='https://127.0.0.1:41617/us-west-2_695cb073a'
(check_cognito_identity__cognito_idp_deep_cognito_identity_3_token_mapping_ambiguous_multi_role_claim_fallback_deny_and_custom_role.py line 288). The host/port in the actual value are per-run environment values; the divergence is the absence of the `cognito-idp.<region>` host labels.

Consequence: the check aborts at the token inspection, so the downstream Cognito Identity assertions never run — Token/AuthenticatedRole fallback to the pool default role, Token/Deny refusal NotAuthorizedException 'The ambiguous role mapping rules for: cognito-idp.<region>.amazonaws.com/<userPoolId> denied this request.', CustomRoleArn honoured when present in cognito:roles, and NotAuthorizedException 'CustomRoleARN: <arn> is not present in the token in the cognito:roles claim' when absent.

## Steps to Reproduce
1) cognito-idp: CreateUserPool, CreateUserPoolClient (GenerateSecret=false, ExplicitAuthFlows=[ALLOW_ADMIN_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH]).
2) AdminCreateUser + AdminSetUserPassword(Permanent=true); create two groups at the same Precedence with distinct RoleArns and add the user to both.
3) AdminInitiateAuth with AuthFlow=ADMIN_USER_PASSWORD_AUTH and base64url-decode AuthenticationResult.IdToken's payload.
4) Read the `iss` claim: it is https://<emulator host>:<port>/<userPoolId> instead of https://cognito-idp.<region>.<domain>/<userPoolId> — the host carries no `cognito-idp` label and no region label.
5) Because the token is rejected as not issued by a Cognito endpoint, the cognito-identity arms (SetIdentityPoolRoles with {Type: Token, AmbiguousRoleResolution: AuthenticatedRole|Deny} then GetCredentialsForIdentity with and without CustomRoleArn) are never reached.
