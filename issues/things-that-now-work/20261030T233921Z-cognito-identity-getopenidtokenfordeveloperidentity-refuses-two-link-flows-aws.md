Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-open-id-token-for-developer-identity-mismatched-identity-id-ignored cognito-identity.merge-developer-identities-preserves-login-insertion-order
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|NotAuthorizedException
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|ResourceConflictException
Issue-Commit: c16f2fd57a71cf90620ee867de88f472739a2612

# cognito-identity: GetOpenIdTokenForDeveloperIdentity refuses two link flows AWS accepts — a mismatched explicit IdentityId is rejected instead of ignored, and adding a developer login to an identity that already holds its own public-provider login is rejected as a link conflict

> _Note: this issue was identified against the code base at commit `c16f2fd57a71cf90620ee867de88f472739a2612`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
REOPENS 20261029T230931Z-cognito-identity-refusals-for-invalid-or-mismatched-logins.md: its fix left the call refused.

That fixed issue added two guards to GetOpenIdTokenForDeveloperIdentity — a logins-vs-identity authorization check (its Cause 9, which must answer NotAuthorizedException "Logins don't match. Please include at least one valid login for this identity or identity pool.") and a link-conflict check (its Cause 6, which must answer ResourceConflictException "Cannot link these logins."). Both guards are now over-broad: they also fire on two link flows that AWS answers HTTP 200, so the operation's normal re-login and login-linking paths are refused outright. This issue bundles 2 root causes, both in cognito-identity's GetOpenIdTokenForDeveloperIdentity handler; both must be closed before submitting.

Smithy (com.amazonaws.cognitoidentity#AWSCognitoIdentityService, awsJson1_1): GetOpenIdTokenForDeveloperIdentity input = IdentityPoolId (required), IdentityId (OPTIONAL, length 1-55, pattern ^[\w-]+:[0-9a-f-]+$), Logins (required, LoginsMap @length min 0 max 10), PrincipalTags, TokenDuration; output = IdentityId + Token; modeled errors = DeveloperUserAlreadyRegisteredException(400), InvalidParameterException(400), NotAuthorizedException(403), ResourceConflictException(409), ResourceNotFoundException(404), TooManyRequestsException(429), InternalErrorException(500). Smithy documentation for the operation: "Registers (or retrieves) a Cognito IdentityId ... Supplying multiple logins will create an implicit linked account. ... You can use GetOpenIdTokenForDeveloperIdentity to create a new identity and to link new logins (that is, user credentials issued by a public provider or developer provider) to an existing identity. When you want to create a new identity, the IdentityId should be null. When you want to associate a new login with an existing authenticated/unauthenticated identity, you can do so by providing the existing IdentityId."

Cause 1 of 2 — an explicit IdentityId that is NOT the identity the developer login is already bound to is refused instead of being ignored.
AWS: recorded behavior only (the model marks IdentityId optional and documents it as the way to name an existing identity; it states no conflict rule). The developer login is canonical: the request succeeds with HTTP 200 and returns the IdentityId the developer user is actually registered under, silently overriding the IdentityId supplied in the request. The named-but-unrelated identity is left untouched — DescribeIdentity on it still shows no developer-provider login, and LookupDeveloperIdentity for the developer user still resolves to the canonical identity.
Emulator: refuses with NotAuthorizedException, HTTP 403 per the model, message "Logins don't match. Please include at least one valid login for this identity or identity pool." — that message is entirely fixed text with no variable parts, and it is AWS's wording for a genuinely non-matching login, which this request is not: the Logins map carries exactly the developer provider configured on the pool and exactly the developer user identifier that is already registered.
Why: this is the ordinary re-login path for an already-registered developer user. Any client that passes a cached, stale or freshly-minted IdentityId alongside the developer login (the SDK credential providers do exactly this) has its working federation flow refused with an authorization error, so no token is ever vended and the caller re-authenticates in a loop against a login that is in fact valid.
Fix: the explicit-IdentityId branch of the GetOpenIdTokenForDeveloperIdentity handler (and the logins-vs-identity guard it feeds) must RESOLVE the developer login to its canonical identity rather than validate the supplied IdentityId against it; the logins-don't-match refusal must be reserved for a Logins map that carries no login valid for the pool/identity.
Traces: cognito-identity.get-open-id-token-for-developer-identity-mismatched-identity-id-ignored.

Cause 2 of 2 — attaching a developer login to an existing identity whose public-provider login is supplied in the SAME request is refused as a link conflict.
AWS: recorded behavior only for the status (the model states no rule), but the operation's own documentation licenses it: supplying multiple logins creates an implicit linked account, and passing the existing IdentityId is the documented way to associate a new login with an existing identity. The call succeeds with HTTP 200 and returns that IdentityId; afterwards the identity's Logins list carries both providers (DescribeIdentity returns both the cognito-idp.<region>.amazonaws.com/<user-pool-id> provider and the pool's DeveloperProviderName).
Emulator: refuses with ResourceConflictException, message "Cannot link these logins." — fixed text, no variable parts. That is AWS's refusal for logins already proven for a DIFFERENT identity; here the public-provider login was proven for the very identity named in IdentityId (it is the identity GetId minted for that same token), and the developer login is new and unbound, so there is no conflicting identity to refuse over.
Why: the documented link-a-new-login flow is unusable. A backend that federates a user-pool user and then attaches its own developer user identifier to the same identity cannot do so at all; every downstream step that depends on the merged identity (MergeDeveloperIdentities, DescribeIdentity's Logins list, credential vending for the linked identity) is unreachable, so the emulator can neither build nor exercise a dual-provider identity.
Fix: the link-conflict check in the GetOpenIdTokenForDeveloperIdentity handler must compare each supplied login's currently-bound identity against the request's target identity and refuse only when a login is bound to a DIFFERENT identity; a login already bound to the target identity (or bound to nothing) must be accepted and appended to that identity's Logins.
Traces: cognito-identity.merge-developer-identities-preserves-login-insertion-order (the check never reaches its merge assertions — the setup call that links the developer login onto the user-pool identity is refused).

Neither cause is covered by any open issue. Related but NOT covering (none carries these checks' replay keys): 20261029T173827Z-cognito-identity-10-net-new-developer-identity-root.md (its Cause 2 is the same check id but keyed on the OLD DeveloperUserAlreadyRegisteredException refusal, which the emulator no longer emits — the refusal is now the logins-don't-match one described in Cause 1 here), and 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md (its Cause 2 is the opposite direction: a public-provider login bound to a DIFFERENT identity, which must be refused).

## Evidence
Behavioral replay, simparity-behavioral-aws-v1 (emulator up and serving: 214/334 checks passed).

Cause 1 — cognito-identity.get-open-id-token-for-developer-identity-mismatched-identity-id-ignored [error]:
  botocore.errorfactory.NotAuthorizedException: An error occurred (NotAuthorizedException) when calling the GetOpenIdTokenForDeveloperIdentity operation: Logins don't match. Please include at least one valid login for this identity or identity pool.
  Sequence: CreateIdentityPool(DeveloperProviderName=<dp>) -> GetOpenIdTokenForDeveloperIdentity(Logins={<dp>: <dev-user>}) = identity A -> GetId(IdentityPoolId=<pool>) = identity B (B != A) -> GetOpenIdTokenForDeveloperIdentity(IdentityId=B, Logins={<dp>: <dev-user>}).
  Expected: HTTP 200 with IdentityId == A (the canonical identity), LookupDeveloperIdentity(<dev-user>) still == A, and DescribeIdentity(B).Logins still without <dp>.
  Actual: HTTP 403 NotAuthorizedException, fixed text "Logins don't match. Please include at least one valid login for this identity or identity pool." (no variable parts); no token or identity returned.

Cause 2 — cognito-identity.merge-developer-identities-preserves-login-insertion-order [error]:
  botocore.errorfactory.ResourceConflictException: An error occurred (ResourceConflictException) when calling the GetOpenIdTokenForDeveloperIdentity operation: Cannot link these logins.
  Sequence: CreateUserPool + CreateUserPoolClient + AdminInitiateAuth -> IdToken; CreateIdentityPool(DeveloperProviderName=<dp>, CognitoIdentityProviders=[{ProviderName: cognito-idp.<region>.amazonaws.com/<user-pool-id>, ClientId: <client>, ServerSideTokenCheck: false}]) -> GetId(Logins={<provider>: <IdToken>}) = dest identity -> GetOpenIdTokenForDeveloperIdentity(IdentityId=<dest>, Logins={<provider>: <IdToken>, <dp>: <dest-user>}).
  Expected: HTTP 200 returning <dest>, and DescribeIdentity(<dest>).Logins containing both <provider> and <dp>.
  Actual: ResourceConflictException, fixed text "Cannot link these logins." (no variable parts). The check's own subject (MergeDeveloperIdentities does not reorder the survivor's pre-existing Logins) is never reached.

Smithy cited: GetOpenIdTokenForDeveloperIdentity.IdentityId is optional (length 1-55, pattern ^[\w-]+:[0-9a-f-]+$); Logins is required (LoginsMap @length min 0 max 10); NotAuthorizedException @httpError 403, ResourceConflictException @httpError 409; operation documentation licenses multiple logins ("implicit linked account") and the existing-IdentityId linking flow.

## Steps to Reproduce
1. Boot the emulator and replay the two checks named in trace_ids.
2. Cause 1 by hand: CreateIdentityPool(AllowUnauthenticatedIdentities=true, DeveloperProviderName=dp). GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=pool, Logins={dp: devuser}) -> record IdentityId A. GetId(IdentityPoolId=pool) -> record IdentityId B. Call GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=pool, IdentityId=B, Logins={dp: devuser}): it must answer HTTP 200 with IdentityId=A and a Token; the emulator answers 403 NotAuthorizedException "Logins don't match. Please include at least one valid login for this identity or identity pool.". Then confirm LookupDeveloperIdentity(IdentityPoolId=pool, DeveloperUserIdentifier=devuser) still returns A and DescribeIdentity(IdentityId=B).Logins does not contain dp.
3. Cause 2 by hand: create a user pool + client, AdminInitiateAuth for an IdToken, and CreateIdentityPool(AllowUnauthenticatedIdentities=false, DeveloperProviderName=dp, CognitoIdentityProviders=[{ProviderName: 'cognito-idp.<region>.amazonaws.com/<user-pool-id>', ClientId: <client>, ServerSideTokenCheck: false}]). GetId(IdentityPoolId=pool, Logins={provider: IdToken}) -> identity D. Call GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=pool, IdentityId=D, Logins={provider: IdToken, dp: destuser}): it must answer HTTP 200 with IdentityId=D; the emulator answers ResourceConflictException "Cannot link these logins.". Then DescribeIdentity(IdentityId=D).Logins must list both provider and dp.
4. Compare HTTP status, body __type / x-amzn-errortype and message against the expectations above for each call.
