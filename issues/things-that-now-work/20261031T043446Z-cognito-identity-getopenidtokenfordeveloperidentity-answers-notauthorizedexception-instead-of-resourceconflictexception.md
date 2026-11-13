Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-merge-idp-proven-identity-login-conflicts
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-merge-idp-proven-identity-login-conflicts
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->4xx|$.message
Issue-Related: 20261029T230931Z-cognito-identity-error-responses-omit-the-x-amzn.md
Issue-Related: 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md
Issue-Commit: f4a3cd4ad22a3104f2bb05cafe64a36012ad514d

# cognito-identity: GetOpenIdTokenForDeveloperIdentity answers NotAuthorizedException instead of ResourceConflictException when a login is already linked to another identity

> _Note: this issue was identified against the code base at commit `f4a3cd4ad22a3104f2bb05cafe64a36012ad514d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
One root cause.

AWS: GetOpenIdTokenForDeveloperIdentity performs an implicit link between the developer provider's user and the identity proven by the other Logins entries. When one of those logins is already linked to a DIFFERENT identity in the pool, AWS refuses the implicit link as a link conflict: HTTP 400 with __type ResourceConflictException and the message "Cannot merge these identities." (Smithy: ResourceConflictException is modeled on GetOpenIdTokenForDeveloperIdentity and documented "Thrown when a user tries to use a login which is already linked to another account"; the model's @httpError for it is 409, but the recorded response for this refusal is 400 — take the status from the recording). The message is entirely fixed text with no variable parts: it does not name the provider, the login, or either identity id. The x-amzn-errortype header carries the same code, 'ResourceConflictException'.

Emulator: the same request is refused with __type NotAuthorizedException and the message "Logins don't match. Please include at least one valid login for this identity or identity pool." (x-amzn-errortype 'NotAuthorizedException:'). The status, 400, already matches. The emulator therefore treats an already-linked login as an invalid/unauthorized login for the target identity rather than as a conflict between two existing identities — the conflict is never detected, and the logins-match authorization check fires first.

Why: ResourceConflictException is the signal a client uses to branch into conflict resolution (pick a surviving identity, call MergeDeveloperIdentities, or surface a 'this account is already linked' choice to the user). NotAuthorizedException reads as a credential/permission failure, so that path never runs; a client sees a retriable-looking auth error for a state condition no retry can clear, and the distinct error code AWS models for exactly this case is unreachable.

Fix: the implicit-link path of the GetOpenIdTokenForDeveloperIdentity handler — before the logins-match authorization check, detect that a supplied public-provider login is already bound to a different identity in the pool and raise the link-conflict error (code ResourceConflictException, fixed message "Cannot merge these identities.") instead of the logins-mismatch refusal.

Traces: cogid-merge-idp-proven-identity-login-conflicts#task#12.

Related open issues that do NOT carry this replay key: 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md, 20261029T173827Z-cognito-identity-10-net-new-developer-identity-root.md, 20261029T230931Z-cognito-identity-identity-scoped-refusals-answer-403-404.md (that one is about refusals whose HTTP STATUS diverges; here the status matches and the code and message diverge).

## Evidence
Fixture cogid-merge-idp-proven-identity-login-conflicts, step task#12, GetOpenIdTokenForDeveloperIdentity, 400 -> 400:
  $.__type: expected 'ResourceConflictException', got 'NotAuthorizedException'
  $.message: expected 'Cannot merge these identities.', got "Logins don't match. Please include at least one valid login for this identity or identity pool."
  $headers.x-amzn-errortype: got 'NotAuthorizedException:'
The expected message is wholly fixed text — no request- or state-derived values. 1 step in the cluster.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from the checkout.
2. Re-replay fixture cogid-merge-idp-proven-identity-login-conflicts, step task#12.
3. By hand: in one identity pool, establish identity A linked to a public-provider login L, then call GetOpenIdTokenForDeveloperIdentity with a developer-provider login for a different user plus Logins containing L. The response must be HTTP 400 with __type ResourceConflictException, message exactly 'Cannot merge these identities.', and x-amzn-errortype naming ResourceConflictException.
