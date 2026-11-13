Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-web-identity-token-outbound-federation-disabled-rejected sts.get-web-identity-token-outbound-federation-disabled-blocked sts.get-web-identity-token-duration-escalation-rejected sts.get-web-identity-token-session-duration-escalation-rejected
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-web-identity-token-outbound-federation-disabled-rejected|run#9|AssertionError|assert code == "OutboundWebIdentityFederationDisabledException", (
Issue-Key: sts.get-web-identity-token-outbound-federation-disabled-blocked|run#13|AssertionError|assert code == "OutboundWebIdentityFederationDisabledException", f"unexpected code: {code}"
Issue-Key: sts.get-web-identity-token-duration-escalation-rejected|run#10|AssertionError|assert code == "SessionDurationEscalationException", f"unexpected code: {code}"
Issue-Key: sts.get-web-identity-token-session-duration-escalation-rejected|run#20|AssertionError|assert code == "SessionDurationEscalationException", (
Issue-Commit: 16ab11b10c0f2146454213d791ef0af113563296

# sts GetWebIdentityToken serves 501 NotImplemented where AWS serves its two modeled 403 refusals (OutboundWebIdentityFederationDisabledException, SessionDurationEscalationException) — 2 causes

> _Note: this issue was identified against the code base at commit `16ab11b10c0f2146454213d791ef0af113563296`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWebIdentityToken

## Description
STS `GetWebIdentityToken` has no handler behaviour at all: it is an unmodified scaffold that answers HTTP 501 `NotImplemented` for every request. The already-open stub issue `20261112T235120Z-sts-getwebidentitytoken-stub.md` asks for the operation to be implemented, but its `Issue-Trace-Ids` carry only the four happy-path/payload checks (`...duration-exceeds-max-rejected`, `...payload-size-exceeded-rejected`, `...returns-valid-jwt-structure`, `...ttl-matches-requested-duration`). The FOUR checks below are not carried by any open issue, and each one asserts a refusal class that a minimal "mint a JWT" implementation would still get wrong, so they are recorded here as their own work with their own replay keys. Two root causes.

Cause 1 of 2: the account-level outbound-web-identity-federation gate is not enforced, so a disabled account is answered 501 instead of the modeled 403.
AWS: Smithy (com.amazonaws.sts#GetWebIdentityToken) models `OutboundWebIdentityFederationDisabledException`, errorKind client, httpError 403, awsQuery error code `OutboundWebIdentityFederationDisabledException`, documented as "The outbound web identity federation feature is not enabled for this account. To use this feature, you must first enable it through the Amazon Web Services Management Console or API." The feature is an account singleton, OFF by default, flipped by IAM `DisableOutboundWebIdentityFederation` / `EnableOutboundWebIdentityFederation` and read back as `GetOutboundWebIdentityFederationInfo.JwtVendingEnabled` (those IAM operations already answer in this emulator — the checks drive them successfully before the STS call). The refusal is server-side: it is returned regardless of how permissive the caller's own IAM policy is. Recorded behavior for the message: the refusal body carries the message `OutboundWebIdentityFederation is disabled.` — that text is entirely fixed, it interpolates no request or state value. Once the account flag is enabled, the identical call must succeed and return `WebIdentityToken` plus `Expiration`.
Emulator: every call, enabled or disabled, is refused with HTTP 501, error code `NotImplemented`, message "operation GetWebIdentityToken is scaffolded but not implemented in this emulator" (observed as botocore `Error.Code == 'NotImplemented'`).
Why: a client that probes the feature flag cannot tell "the account has outbound federation turned off" from "this API does not exist": `NotImplemented` is not in the operation's modeled error set, so SDK error-specific handling (`except sts.exceptions.OutboundWebIdentityFederationDisabledException`) never matches, and a 501 is classed as a server-side/transient failure rather than the permanent, actionable configuration refusal AWS returns. Code that enables the feature and retries, and code that treats the disabled state as a 403 to surface to an operator, both break.
Fix: the operation's handler plus its authorization/validation step, `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_web_identity_token.rs` — before minting anything, read the account-level outbound-web-identity-federation flag that the IAM Enable/Disable operations write and return the modeled `OutboundWebIdentityFederationDisabledException` (403, with the fixed message above) when it is off.
Traces: sts.get-web-identity-token-outbound-federation-disabled-rejected, sts.get-web-identity-token-outbound-federation-disabled-blocked.

Cause 2 of 2: a requested JWT duration that outlives the caller's own session is not refused — no session-lifetime cap exists, and the call is answered 501.
AWS: Smithy models `SessionDurationEscalationException`, errorKind client, httpError 403, awsQuery error code `SessionDurationEscalationException`, documented as "The requested token duration would extend the session beyond its original expiration time. You cannot use this operation to extend the lifetime of a session beyond what was granted when the session was originally created." `DurationSeconds` is itself range 60..3600 with a default of 300, so an in-range duration must still be rejected when it exceeds the caller's REMAINING session lifetime — the comparison is against the calling credential's expiry, not against the modeled maximum. Recorded behavior for the message: the refusal body carries `Requested token expiry time must be before the original session's expiry time.` — fixed text, no interpolated values. The rule's variable parts are the two times being compared (the requested expiry = now + DurationSeconds, and the caller's session expiry), which come from the request and from emulator session state. Positive control from the same session: a `DurationSeconds` inside the remaining lifetime (60s from a 900s assumed-role session) must succeed and return a `WebIdentityToken`.
Emulator: the 3600s-from-a-900s-session call is refused with HTTP 501 / `NotImplemented` (same scaffold message), and so is the 60s positive control, so neither the refusal nor the allowed case is served.
Why: a caller relying on AWS's guarantee that a web-identity JWT can never outlive the session that minted it gets no refusal at all here; once the stub is replaced by a naive "mint for DurationSeconds" handler, the same clients would receive a token valid past their session's expiry, which is a privilege-escalation divergence a downstream OIDC verifier would honour. In the current state the error code is also unmodeled, so `except sts.exceptions.SessionDurationEscalationException` never matches.
Fix: the same handler and its validation step in `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_web_identity_token.rs` — resolve the calling credential's session expiry from STS session state and refuse with the modeled `SessionDurationEscalationException` (403, fixed message above) whenever now + `DurationSeconds` is not before it; otherwise mint with `Expiration` = issue time + `DurationSeconds`.
Traces: sts.get-web-identity-token-duration-escalation-rejected, sts.get-web-identity-token-session-duration-escalation-rejected.

## Evidence
Cause 1 — sts.get-web-identity-token-outbound-federation-disabled-rejected:
  AssertionError: expected OutboundWebIdentityFederationDisabledException while the feature is disabled, got 'NotImplemented'
  (check line 44; the check had already driven IAM DisableOutboundWebIdentityFederation successfully and polled for the refusal to settle)
  Expected: HTTP 403, Error.Code 'OutboundWebIdentityFederationDisabledException', Error.Message 'OutboundWebIdentityFederation is disabled.'; then, after re-enabling, HTTP 200 with WebIdentityToken/Expiration.
  Actual:   HTTP 501, Error.Code 'NotImplemented', Error.Message 'operation GetWebIdentityToken is scaffolded but not implemented in this emulator'.

Cause 1 — sts.get-web-identity-token-outbound-federation-disabled-blocked:
  AssertionError: unexpected code: NotImplemented (check line 72; asserts the refusal is server-side, i.e. independent of the caller's IAM permissions).

Cause 2 — sts.get-web-identity-token-duration-escalation-rejected:
  AssertionError: unexpected code: NotImplemented (check line 49)
  Expected: from a 900s assumed-role session, DurationSeconds=3600 -> HTTP 403, Error.Code 'SessionDurationEscalationException', Error.Message 'Requested token expiry time must be before the original session's expiry time.'; and DurationSeconds=60 from the same session -> HTTP 200 with a WebIdentityToken.
  Actual:   both calls -> HTTP 501, Error.Code 'NotImplemented'.

Cause 2 — sts.get-web-identity-token-session-duration-escalation-rejected:
  AssertionError: expected SessionDurationEscalationException for a JWT duration exceeding the caller's remaining session lifetime, got 'NotImplemented' (check line 95).

Source: services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_web_identity_token.rs line 189 returns crate::scaffolded::Scaffolded::not_implemented("GetWebIdentityToken"); the modeled error channel (lines 118-124) already declares JWTPayloadSizeExceededException / OutboundWebIdentityFederationDisabledException / SessionDurationEscalationException, so no model work is needed to return either refusal.

Related open issue (does NOT carry these four replay keys): 20261112T235120Z-sts-getwebidentitytoken-stub.md, whose Issue-Trace-Ids are sts.get-web-identity-token-duration-exceeds-max-rejected, sts.get-web-identity-token-payload-size-exceeded-rejected, sts.get-web-identity-token-returns-valid-jwt-structure, sts.get-web-identity-token-ttl-matches-requested-duration.

## Steps to Reproduce
Cause 1 (account-level gate):
1. Boot the emulator and create a client for sts and iam.
2. iam.disable_outbound_web_identity_federation(); poll iam.get_outbound_web_identity_federation_info() until JwtVendingEnabled is false.
3. sts.get_web_identity_token(Audience=['https://example.invalid'], SigningAlgorithm='RS256').
   Expect HTTP 403 / OutboundWebIdentityFederationDisabledException with message 'OutboundWebIdentityFederation is disabled.'; observe HTTP 501 / NotImplemented.
4. iam.enable_outbound_web_identity_federation() and repeat step 3: expect HTTP 200 with WebIdentityToken and Expiration.
5. Repeat step 3 with a caller whose identity policy allows sts:GetWebIdentityToken on * — the refusal must be unchanged (server-side), which is what the ...-disabled-blocked check asserts.

Cause 2 (session-lifetime cap):
1. Enable outbound web identity federation as above.
2. Create a role trusting the caller, attach an inline policy allowing sts:GetWebIdentityToken on *, and sts.assume_role(..., DurationSeconds=900).
3. With the returned temporary credentials, call sts.get_web_identity_token(Audience=['https://example.invalid'], SigningAlgorithm='RS256', DurationSeconds=3600).
   Expect HTTP 403 / SessionDurationEscalationException with message "Requested token expiry time must be before the original session's expiry time."; observe HTTP 501 / NotImplemented.
4. Repeat with DurationSeconds=60 on the same session: expect HTTP 200 and a WebIdentityToken whose Expiration is issue time + 60s and before the session's own expiry.

Replay: the four check ids in trace_ids.
