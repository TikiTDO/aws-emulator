Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.op-contract-update-auth-event-feedback
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.op-contract-update-auth-event-feedback|run#18|AssertionError|assert code2 == "NotAuthorizedException", code2
Issue-Commit: c45cac5125c9560fdc1175eb4c5430e7f1b63b55

# cognito-idp UpdateAuthEventFeedback resolves Username before the FeedbackToken, leaking UserNotFoundException instead of NotAuthorizedException 'Invalid session provided'

> _Note: this issue was identified against the code base at commit `c45cac5125c9560fdc1175eb4c5430e7f1b63b55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAuthEventFeedback

## Description
One net-new cause in cognito-idp, grounded in behavioral check cognito-idp.op-contract-update-auth-event-feedback. Related open issue 20261112T145631Z-cognito-idp-auth-event-feedback-refusals-leak-usernotfoundexception.md describes the same fault from the WIRE corpus (its Cause 2), but it carries only wire replay keys (cognito-idp|UpdateAuthEventFeedback|4xx->4xx|...) and no behavioral check key, so this behavioral check is not covered by it; a fixer closing that issue must also re-replay the check named below.

Cause 1 of 1: UpdateAuthEventFeedback on a Plus-tier pool with threat protection (AdvancedSecurityMode) enabled, called with a FeedbackToken that authorizes nothing and a Username that names no user of the pool, is refused with UserNotFoundException instead of the token/session refusal NotAuthorizedException. Failing check: cognito-idp.op-contract-update-auth-event-feedback.

AWS: the refusal is HTTP 400 with error code NotAuthorizedException (x-amzn-errortype 'NotAuthorizedException:') and message 'Invalid session provided'. That message is entirely FIXED text for this error class — it interpolates no value from the request or from state (no username, no pool id, no event id, no token). Smithy: UpdateAuthEventFeedback is @optionalAuth with an empty auth list (it is authorized by the request's FeedbackToken — a @sensitive TokenModelType, @pattern ^[A-Za-z0-9-_=.]+$ — not by SigV4), and declares both NotAuthorizedException and UserNotFoundException among its errors; its own doc states "Because FeedbackToken is a required parameter, you can't make requests to UpdateAuthEventFeedback without the contents of the notification email message". The model does not state the evaluation ORDER, so for the ordering itself: recorded behavior only — the recording shows AWS answers on the token/session and never discloses whether the user exists (a user that does not exist can hold no valid feedback token). The HTTP status is NOT what diverges: AWS answers 400 here (not the model's defaulted 403 on NotAuthorizedException), and the emulator also answers 400.

Emulator: HTTP 400 with error code UserNotFoundException and message 'User does not exist.' — the user lookup happens first and its refusal is surfaced verbatim, so the existence of the user is disclosed and a different error class is reported. The same check's earlier assertions pass: the pool-level gate (UserPoolAddOnNotEnabledException with message 'This is an add on feature. Please update AdvancedSecurityMode for your user pool to access this API.') is already correct for both UpdateAuthEventFeedback and AdminListUserAuthEvents; only this token-vs-user ordering diverges.

Why: this is an UNAUTHENTICATED entry point (optionalAuth — anyone holding a notification link can call it), so the divergence hands an anonymous caller a pool-membership oracle, which AWS's single fixed 'Invalid session provided' refusal exists to prevent: on AWS the refusal is byte-identical for an existing and a non-existing user, on the emulator it is not. A client implementing the one-click notification-link flow also branches on the code: UserNotFoundException reads as 'provision this user / stop retrying', NotAuthorizedException reads as 'this link is expired or forged', so the end user is shown the wrong message and automation takes the wrong path.

Fix: the operation's handler/validation — the FeedbackToken check must gate the response before the username (and event) is resolved, and any request whose Username/EventId cannot be bound to a valid token must return the NotAuthorizedException variant the handler already builds, with AWS's fixed 'Invalid session provided' text, instead of the pool-level user_not_found() refusal. The correct error class already exists in the handler but is unreachable for an unknown username.

Traces: cognito-idp.op-contract-update-auth-event-feedback (file and line citations under Evidence).

## Evidence
Failing behavioral check cognito-idp.op-contract-update-auth-event-feedback ([mismatch]):

  File "check_cognito_idp_op_contract_update_auth_event_feedback.py", line 77, in run
    assert code2 == "NotAuthorizedException", code2
  AssertionError: UserNotFoundException

The check creates a pool with UserPoolTier=PLUS and UserPoolAddOns={'AdvancedSecurityMode':'AUDIT'}, then calls UpdateAuthEventFeedback(UserPoolId=<that pool>, Username='no-such-user-ever', EventId=<a well-formed event id>, FeedbackToken=<a token matching ^[A-Za-z0-9-_=.]+$ that authorizes nothing>, FeedbackValue='Valid') and asserts:
  error code  : expected 'NotAuthorizedException', got 'UserNotFoundException'
  error message: expected 'Invalid session provided', got 'User does not exist.'
  HTTP status : 400 on both sides — the status does not diverge.
The check then confirms via AdminGetUser that the username really is absent (UserNotFoundException / 'User does not exist.'), i.e. AWS's refusal above provably came from token validation, not from user resolution.

Earlier assertions of the same check PASS, so they are not part of this defect: UpdateAuthEventFeedback and AdminListUserAuthEvents on a pool WITHOUT advanced security both answer UserPoolAddOnNotEnabledException with the fixed message 'This is an add on feature. Please update AdvancedSecurityMode for your user pool to access this API.'

Emulator source (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_auth_event_feedback.rs):
  line 176: `let user = pool.user_mut(&input.username).ok_or_else(user_not_found)?;` — resolves the user FIRST.
  lines 209-214: `user_not_found()` builds UserNotFoundException { message: crate::state::USER_NOT_FOUND_MESSAGE } ("User does not exist.").
  line 179: event lookup, refused with ResourceNotFoundException via `event_not_found()` (lines 218-223) — also reached before the token check.
  line 184: `if !event.authorizes_feedback(&input.feedback_token) { return Err(feedback_token_rejected()); }` — the token check runs LAST.
  lines 229-234: `feedback_token_rejected()` already returns NotAuthorizedException with crate::state::auth_events::FEEDBACK_TOKEN_INVALID_MESSAGE — the correct error class is present but unreachable for an unknown username or unknown event id.

Same fault, wire corpus: open issue 20261112T145631Z-cognito-idp-auth-event-feedback-refusals-leak-usernotfoundexception.md Cause 2 (fixtures cognito-idp-risk-configuration-and-auth-event-feedback#task#6, cognito-idp-update-auth-event-feedback-errors#task#2). That issue carries no behavioral replay key for this check.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Run check cognito-idp.op-contract-update-auth-event-feedback.
3. Minimal reproduction with the AWS CLI/botocore against the emulator:
   a. CreateUserPool(PoolName=<any>, UserPoolTier='PLUS', UserPoolAddOns={'AdvancedSecurityMode':'AUDIT'}) -> note the pool id.
   b. UpdateAuthEventFeedback(UserPoolId=<pool id>, Username='no-such-user-ever', EventId='00000000-0000-0000-0000-000000000000', FeedbackToken='bogus-token', FeedbackValue='Valid').
      AWS: HTTP 400, error code NotAuthorizedException, message 'Invalid session provided'.
      Emulator: HTTP 400, error code UserNotFoundException, message 'User does not exist.'.
   c. AdminGetUser(UserPoolId=<pool id>, Username='no-such-user-ever') -> UserNotFoundException / 'User does not exist.', confirming the username is absent and that AWS's refusal in (b) is a token refusal.
4. Cross-check the masking the fix must restore: create a user in the pool, then repeat (b) for that EXISTING username with the same bogus FeedbackToken and an unknown EventId. On AWS the refusal is indistinguishable from the unknown-username one (400 NotAuthorizedException 'Invalid session provided'); confirm the emulator answers identically in both cases so no membership can be inferred.
5. Confirm the already-correct behavior did not regress: on a pool WITHOUT advanced security, UpdateAuthEventFeedback and AdminListUserAuthEvents must still answer UserPoolAddOnNotEnabledException with 'This is an add on feature. Please update AdvancedSecurityMode for your user pool to access this API.'
