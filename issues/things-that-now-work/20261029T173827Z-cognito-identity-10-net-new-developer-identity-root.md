Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.developer-user-already-registered-rejected cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-succeeds cognito-identity.get-open-id-token-for-developer-identity-mismatched-identity-id-ignored cognito-identity.get-open-id-token-for-developer-identity-missing-identity-and-provider-rejected cognito-identity.get-open-id-token-for-developer-identity-missing-required-login-rejected cognito-identity.developer-provider-must-be-configured-on-pool cognito-identity.get-open-id-token-for-developer-identity-cross-pool-identity-id-rejected cognito-identity.e2e-identity-pool-mismatch-rejected cognito-identity.lookup-developer-identity-empty-result-no-error cognito-identity.lookup-developer-identity-mismatch-rejected cognito-identity.lookup-developer-identity-missing-identifier-rejected cognito-identity.op-contract-lookupDeveloperIdentity cognito-identity.op-contract-unlinkDeveloperIdentity cognito-identity.unlink-developer-identity-not-linked-is-an-idempotent-no-op cognito-identity.unlink-identity-unknown-identity-rejected cognito-identity.unlink-identity-unknown-login-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.developer-user-already-registered-rejected|run#13|AssertionError|assert err == ("NotAuthorizedException", "Logins don't match. Please include at least one valid login for this identity or identity pool."), f"got {err!r}"
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected|run#11|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict|run#9|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-succeeds|run#16|AssertionError|assert resolved_identity == identity_a, resolved_identity
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|DeveloperUserAlreadyRegisteredException
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-missing-identity-and-provider-rejected|run#8|AssertionError|assert message == "Please specify at least one login provider", message
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-missing-required-login-rejected|run#10|AssertionError|assert message == "This identity pool does not support the specified developer provider.", (
Issue-Key: cognito-identity.developer-provider-must-be-configured-on-pool|run#9|AssertionError|assert err == ("InvalidParameterException", "This identity pool does not support the specified developer provider."), (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-cross-pool-identity-id-rejected|run#15|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.e2e-identity-pool-mismatch-rejected|run#11|AssertionError|assert err == ("NotAuthorizedException", f"Access to Identity '{identity_id}' is forbidden."), f"expected NotAuthorizedException, got {err!r}"
Issue-Key: cognito-identity.lookup-developer-identity-empty-result-no-error|run#5|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.lookup-developer-identity-mismatch-rejected|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.lookup-developer-identity-missing-identifier-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Key: cognito-identity.op-contract-lookupDeveloperIdentity|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-identity|UnlinkDeveloperIdentity|NotAuthorizedException
Issue-Key: cognito-identity.unlink-identity-unknown-identity-rejected|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.unlink-identity-unknown-login-rejected|run#16|AssertionError|assert code == "NotAuthorizedException", f"CODE={code} MESSAGE={message!r}"
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cognito-identity: 10 net-new developer-identity root causes — GetOpenIdTokenForDeveloperIdentity, LookupDeveloperIdentity and Unlink* diverge in classification, message and effect

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
Ten distinct root causes in the cognito-identity developer-identity surface. Each is a separate work item; all ten must be closed before submitting. Related but NOT covering issues already open: 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md and 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md (neither carries the replay keys below).

Cause 1 of 10 — GetOpenIdTokenForDeveloperIdentity re-linking an already-registered developer user onto a DIFFERENT developer identity.
AWS: recorded behavior only (the model permits both errors): the request is an authorization failure, NotAuthorizedException (Smithy: NotAuthorizedException, errorKind client, @httpError 403), message fixed text "Logins don't match. Please include at least one valid login for this identity or identity pool." (no variable parts).
Emulator: DeveloperUserAlreadyRegisteredException (HTTP 400), message "The developer user identifier '<dev-user>' is already registered under identity '<identity-id>'." — fixed text with the developer user identifier and identity id taken from request/state.
Why: a client that branches on NotAuthorizedException (re-authenticate / deny) never sees it and instead gets a conflict code AWS does not use here, so a credential-mismatch path is mis-handled as a resource conflict and is retried instead of refused.
Fix: the GetOpenIdTokenForDeveloperIdentity handler's developer-user link resolution and its error mapping in the cognito-identity service crate.
Traces: cognito-identity.developer-user-already-registered-rejected, cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected, cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict.

Cause 2 of 10 — GetOpenIdTokenForDeveloperIdentity with an explicit IdentityId that carries NO developer login of its own.
AWS: recorded behavior only: the mismatched IdentityId is IGNORED; the developer login is resolved back to the identity it is actually linked to and that canonical IdentityId is returned with HTTP 200; the named identity is left untouched (DescribeIdentity on it still shows no developer-provider login).
Emulator: refuses the call with DeveloperUserAlreadyRegisteredException (HTTP 400, "The developer user identifier '<dev-user>' is already registered under identity '<identity-id>'.").
Why: the normal re-login path for an existing developer user breaks whenever the client passes a stale or freshly-minted IdentityId, so a working federation flow is refused outright.
Fix: same handler as Cause 1 — the explicit-IdentityId branch must resolve, not validate-and-reject.
Traces: cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-succeeds, cognito-identity.get-open-id-token-for-developer-identity-mismatched-identity-id-ignored.

Cause 3 of 10 — GetOpenIdTokenForDeveloperIdentity Logins validation collapses two distinct AWS refusals into one invented message.
AWS: InvalidParameterException (Smithy: @httpError 400) in both cases, with two different fixed texts and no variable parts: for Logins={} the message is "Please specify at least one login provider"; for a Logins map keyed by a provider the pool is not configured with, it is "This identity pool does not support the specified developer provider."
Emulator: one message for both shapes — "Invalid value for parameter Logins: it must carry an entry for the developer provider '<developer-provider>' of identity pool '<pool-id>'." (its variable parts are the pool's configured provider name and the pool id from state). The code, InvalidParameterException, is right.
Why: callers and SDK-level assertions that distinguish "no login supplied" from "wrong provider supplied" cannot tell the two apart, and no client string-matching AWS's wording recognises either.
Fix: the Logins validation of that handler — two separate refusals with AWS's fixed texts.
Traces: cognito-identity.get-open-id-token-for-developer-identity-missing-identity-and-provider-rejected, cognito-identity.get-open-id-token-for-developer-identity-missing-required-login-rejected, cognito-identity.developer-provider-must-be-configured-on-pool.

Cause 4 of 10 — an IdentityId belonging to a FOREIGN identity pool is treated as a missing resource instead of an ownership failure.
AWS: recorded behavior only: ownership is checked before the identity is looked up in the named pool's scope, so the call is refused NotAuthorizedException (@httpError 403) with message "Access to Identity '<identity-id>' is forbidden." — fixed text around the identity id from the request.
Emulator: ResourceNotFoundException with message "Identity '<identity-id>' not found in identity pool '<pool-id>'."
Why: a cross-tenant/cross-pool identity leak reads to the caller as a 'not found' (retryable/absent) rather than a denial, and the pool-scoped ownership check is observably absent.
Fix: the identity-ownership precheck shared by GetOpenIdTokenForDeveloperIdentity (and the LookupDeveloperIdentity path it feeds) in the cognito-identity state layer.
Traces: cognito-identity.get-open-id-token-for-developer-identity-cross-pool-identity-id-rejected, cognito-identity.e2e-identity-pool-mismatch-rejected.

Cause 5 of 10 — LookupDeveloperIdentity succeeds with an empty list for an identity that has no active developer link.
AWS: recorded behavior only: LookupDeveloperIdentity only succeeds for a currently-active identity/developer-user link; for an identity that never had (or no longer has) a developer login it is refused NotAuthorizedException (@httpError 403) with the fixed message "You do not have access to this identity" — AWS does not distinguish "no developer login" from "no access".
Emulator: HTTP 200 with IdentityId echoed and DeveloperUserIdentifierList: [].
Why: an over-permissive success turns a denial into a legitimate empty result, so code that treats an empty list as "user has no developer identity" draws a conclusion AWS never licenses.
Fix: the LookupDeveloperIdentity handler's link lookup — absent link must refuse, not return an empty page.
Traces: cognito-identity.lookup-developer-identity-empty-result-no-error.

Cause 6 of 10 — LookupDeveloperIdentity pair-mismatch message.
AWS: recorded behavior only: fixed text "The provided user identifier and identity id do not match." (no variable parts).
Emulator: "Developer user identifier '<dev-user>' is not registered under identity '<identity-id>'."
Why: clients matching AWS's wording to classify a mismatch see an unknown message.
Fix: the LookupDeveloperIdentity handler's refusal message.
Traces: cognito-identity.lookup-developer-identity-mismatch-rejected.

Cause 7 of 10 — LookupDeveloperIdentity with neither IdentityId nor DeveloperUserIdentifier.
AWS: InvalidParameterException (@httpError 400) with the fixed text "Please specify either an identity id or developer user identifier" (no variable parts; both members are optional in the model — Smithy: LookupDeveloperIdentity.IdentityId and .DeveloperUserIdentifier are not @required — so this is a handler-level rule).
Emulator: InvalidParameterException with "Invalid value for parameters IdentityId and DeveloperUserIdentifier: at least one of them must be supplied."
Why: the code matches but the message does not, so AWS-worded assertions and user-facing error surfaces diverge.
Fix: the LookupDeveloperIdentity input validation message.
Traces: cognito-identity.lookup-developer-identity-missing-identifier-rejected, cognito-identity.op-contract-lookupDeveloperIdentity.

Cause 8 of 10 — UnlinkDeveloperIdentity refuses a pair that is not linked instead of being an idempotent no-op.
AWS: recorded behavior only: unlinking a developer provider/user pair that is not currently linked to the identity is an idempotent no-op — HTTP 200, empty body (Smithy: UnlinkDeveloperIdentity output is Unit).
Emulator: NotAuthorizedException (@httpError 403) with the fixed text "You do not have access to this identity".
Why: cleanup and retry paths (which re-issue Unlink after a partial failure) fail against the emulator where they succeed on AWS, and the refusal also masks the real authorization error of the same name.
Fix: the UnlinkDeveloperIdentity handler — missing link must be a no-op.
Traces: cognito-identity.op-contract-unlinkDeveloperIdentity, cognito-identity.unlink-developer-identity-not-linked-is-an-idempotent-no-op.

Cause 9 of 10 — UnlinkIdentity accepts an unknown IdentityId.
AWS: ResourceNotFoundException (Smithy: ResourceNotFoundException is a modeled error of UnlinkIdentity) with message "Identity '<identity-id>' not found." — fixed text around the identity id from the request.
Emulator: HTTP 200 with an empty body; nothing is refused.
Why: an over-permissive success: a client that deletes/unlinks by id gets no signal that the id does not exist, so a typo or a stale id silently 'succeeds'.
Fix: the UnlinkIdentity handler's identity lookup.
Traces: cognito-identity.unlink-identity-unknown-identity-rejected.

Cause 10 of 10 — UnlinkIdentity with no Logins supplied is classified as a parameter error instead of a denial.
AWS: recorded behavior only: supplying no Logins for the identity (with any LoginsToRemove, including an empty list) is refused NotAuthorizedException (@httpError 403) with message "Access to Identity '<identity-id>' is forbidden." — the identity id comes from the request; the refusal is about the missing Logins, not about the provider named in LoginsToRemove.
Emulator: InvalidParameterException with the fixed text "Please provide a valid public provider".
Why: an unauthenticated unlink attempt is reported as a client parameter mistake, so callers retry with different parameters instead of re-authenticating, and the authorization gate is invisible.
Fix: the UnlinkIdentity handler — authorize the caller against the identity's logins before validating LoginsToRemove.
Traces: cognito-identity.unlink-identity-unknown-login-rejected.

## Evidence
Cause 1: expected ('NotAuthorizedException', "Logins don't match. Please include at least one valid login for this identity or identity pool."); got ('DeveloperUserAlreadyRegisteredException', "The developer user identifier 'user-9cf9efbfbd7c' is already registered under identity 'us-west-2:b06daac5-...'.").
Cause 2: expected HTTP 200 returning the canonical identity (identity A); got botocore.errorfactory.DeveloperUserAlreadyRegisteredException on GetOpenIdTokenForDeveloperIdentity.
Cause 3: expected ('InvalidParameterException', 'Please specify at least one login provider') for Logins={} and ('InvalidParameterException', 'This identity pool does not support the specified developer provider.') for a foreign provider key; got for BOTH "Invalid value for parameter Logins: it must carry an entry for the developer provider 'sim2real-dev-provider-...' of identity pool 'us-west-2:...'.".
Cause 4: expected ('NotAuthorizedException', "Access to Identity 'us-west-2:<id>' is forbidden."); got ('ResourceNotFoundException', "Identity 'us-west-2:<id>' not found in identity pool 'us-west-2:<pool>'.").
Cause 5: expected ('NotAuthorizedException', 'You do not have access to this identity'); got HTTP 200 {'IdentityId': 'us-west-2:f08aa172-...', 'DeveloperUserIdentifierList': []}.
Cause 6: expected 'The provided user identifier and identity id do not match.'; got "Developer user identifier 'user-a-7827d8817ac4' is not registered under identity 'us-west-2:760ed36b-...'.".
Cause 7: expected 'Please specify either an identity id or developer user identifier'; got 'Invalid value for parameters IdentityId and DeveloperUserIdentifier: at least one of them must be supplied.'.
Cause 8: expected HTTP 200/empty; got botocore.errorfactory.NotAuthorizedException: 'You do not have access to this identity' on UnlinkDeveloperIdentity.
Cause 9: expected ('ResourceNotFoundException', "Identity '<ghost-id>' not found."); got HTTP 200 with an empty body.
Cause 10: expected ('NotAuthorizedException', "Access to Identity '<identity-id>' is forbidden."); got CODE=InvalidParameterException MESSAGE='Please provide a valid public provider'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay exactly the checks named in trace_ids (each cause's checks are listed under its 'Traces:' line) — e.g. cognito-identity.developer-user-already-registered-rejected for Cause 1, cognito-identity.unlink-identity-unknown-login-rejected for Cause 10.
3. By hand for Cause 2: CreateIdentityPool with DeveloperProviderName=dp; GetOpenIdTokenForDeveloperIdentity(Logins={dp: user}) -> identity A; GetId(IdentityPoolId=pool) -> identity B; GetOpenIdTokenForDeveloperIdentity(IdentityId=B, Logins={dp: user}) must return IdentityId=A with HTTP 200 and leave B's Logins empty.
4. By hand for Cause 9: UnlinkIdentity(IdentityId='us-west-2:00000000-0000-0000-0000-000000000000', Logins={...}, LoginsToRemove=[...]) must answer ResourceNotFoundException "Identity '<id>' not found.", not 200.
