Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-auth-event-feedback-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-auth-event-feedback-errors cognito-idp-risk-configuration-and-auth-event-feedback cognito-idp-update-auth-event-feedback-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminUpdateAuthEventFeedback|4xx->4xx|$.__type
Issue-Key: cognito-idp|AdminUpdateAuthEventFeedback|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminUpdateAuthEventFeedback|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateAuthEventFeedback|4xx->4xx|$.__type
Issue-Key: cognito-idp|UpdateAuthEventFeedback|4xx->4xx|$.message
Issue-Key: cognito-idp|UpdateAuthEventFeedback|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: b66c1efa9e88e06042b7a9bd3b7309ec99d3bbd2

# cognito-idp auth-event feedback refusals leak UserNotFoundException instead of NotAuthorizedException

> _Note: this issue was identified against the code base at commit `b66c1efa9e88e06042b7a9bd3b7309ec99d3bbd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminUpdateAuthEventFeedback

## Description
Two net-new causes (both cognito-idp), one shared fault: both auth-event-feedback operations resolve the request's Username against pool state FIRST and surface the state lookup's UserNotFoundException/"User does not exist." verbatim, where AWS refuses the call as NotAuthorizedException without ever disclosing whether the user exists. No open issue carries either replay key; the nearest neighbours in the cognito-idp refusal family (20261102T111936Z-cognito-idp-user-pool-refusal-messages-and-error.md and 20261111T204833Z-cognito-idp-6-net-new-causes-hosted-ui.md) are about user-pool/app-client and hosted-UI refusals and do not touch either of these operations.

Cause 1 of 2: AdminUpdateAuthEventFeedback for a Username that names no user of the pool answers UserNotFoundException instead of NotAuthorizedException.
AWS: the refusal is HTTP 400 with __type 'NotAuthorizedException', header x-amzn-errortype 'NotAuthorizedException:' and message 'Incorrect username or password.' — the whole message is fixed text for this error class; it carries no value from the request or from state (no username, no pool id, no event id are interpolated). Smithy: AdminUpdateAuthEventFeedback declares BOTH NotAuthorizedException (client) and UserNotFoundException (client) among its errors, and its input members UserPoolId/Username/EventId/FeedbackValue are all @required; the model does not say which of the two a username miss selects, so for the selection itself: recorded behavior only — the recording shows AWS answers the credential-shaped NotAuthorizedException. Note the HTTP status is NOT what diverges: both AWS and the emulator answer 400 here (the model's defaulted 403 on NotAuthorizedException is not what Cognito puts on the wire for this refusal).
Emulator: HTTP 400 with __type 'UserNotFoundException', x-amzn-errortype 'UserNotFoundException:' and message 'User does not exist.' — i.e. the existence of the user is disclosed, and with a different error code.
Why: a client that branches on the error code takes the wrong path — UserNotFoundException reads as "provision this user" or "stop retrying", while NotAuthorizedException reads as "the caller/credentials were refused" — so admin tooling that reconciles threat-protection feedback either creates users AWS would not have it create or aborts a loop AWS would have it continue. It is also a user-enumeration oracle: against AWS this refusal is identical for an existing and a non-existing user, and against the emulator it is not, so a probe can enumerate pool membership.
Fix: the operation's handler/validation in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_update_auth_event_feedback.rs — the username lookup miss must map to the NotAuthorizedException variant the operation already declares, carrying AWS's fixed text, instead of the shared crate::state::USER_NOT_FOUND_MESSAGE UserNotFoundException.
Traces: cognito-idp-admin-auth-event-feedback-errors#task#2, cognito-idp-risk-configuration-and-auth-event-feedback#task#5.

Cause 2 of 2: UpdateAuthEventFeedback (the user's own, FeedbackToken-authorized variant) for a Username that names no user answers UserNotFoundException instead of NotAuthorizedException on the session/token.
AWS: the refusal is HTTP 400 with __type 'NotAuthorizedException', header x-amzn-errortype 'NotAuthorizedException:' and message 'Invalid session provided' — again entirely fixed text for this error class, with no request or state value interpolated. Smithy: UpdateAuthEventFeedback has @optionalAuth with an empty auth list (it is authorized by the request's FeedbackToken, a @sensitive TokenModelType @pattern ^[A-Za-z0-9-_=.]+$, not by SigV4) and declares NotAuthorizedException among its errors; its doc states "Because FeedbackToken is a required parameter, you can't make requests to UpdateAuthEventFeedback without the contents of the notification email message". Recorded behavior only for the ordering: because a non-existent user can hold no valid feedback token, AWS answers on the token/session and never on user existence.
Emulator: HTTP 400 with __type 'UserNotFoundException', x-amzn-errortype 'UserNotFoundException:' and message 'User does not exist.'.
Why: this is an UNAUTHENTICATED entry point (optionalAuth — anyone holding a link can call it), so the divergence hands an anonymous caller a membership oracle for the pool, which is exactly what AWS's single 'Invalid session provided' refusal prevents. A client implementing the notification-link flow also mis-classifies an expired/forged link as a missing user and shows the wrong message to the end user.
Fix: the operation's handler/validation in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_auth_event_feedback.rs — the FeedbackToken check must gate the response before user resolution, and a username (or event) that cannot be bound to a valid token must return NotAuthorizedException with AWS's fixed 'Invalid session provided' text rather than the user_not_found() refusal raised at the pool.user_mut lookup.
Traces: cognito-idp-risk-configuration-and-auth-event-feedback#task#6, cognito-idp-update-auth-event-feedback-errors#task#2.

## Evidence
Cause 1 — AdminUpdateAuthEventFeedback, fixture cognito-idp-admin-auth-event-feedback-errors step task#2 (400 -> 400, cluster of 2 steps, also cognito-idp-risk-configuration-and-auth-event-feedback#task#5):
  $.__type: expected 'NotAuthorizedException', got 'UserNotFoundException'
  $.message: expected 'Incorrect username or password.', got 'User does not exist.'
  $headers.x-amzn-errortype: expected 'NotAuthorizedException:', got 'UserNotFoundException:'
  Emulator source: services/.../src/ops/admin_update_auth_event_feedback.rs line 170 `pool.user_mut(&input.username).ok_or_else(user_not_found)?` -> user_not_found() at lines 202-207 builds UserNotFoundException { message: crate::state::USER_NOT_FOUND_MESSAGE }.

Cause 2 — UpdateAuthEventFeedback, fixture cognito-idp-risk-configuration-and-auth-event-feedback step task#6 (400 -> 400, cluster of 2 steps, also cognito-idp-update-auth-event-feedback-errors#task#2):
  $.__type: expected 'NotAuthorizedException', got 'UserNotFoundException'
  $.message: expected 'Invalid session provided', got 'User does not exist.'
  $headers.x-amzn-errortype: expected 'NotAuthorizedException:', got 'UserNotFoundException:'
  Emulator source: services/.../src/ops/update_auth_event_feedback.rs line 176 resolves the user (user_not_found(), lines 209-214) BEFORE the token check at line 184 `event.authorizes_feedback(&input.feedback_token)`, whose own refusal path feedback_token_rejected() (lines 229-234) already returns NotAuthorizedException — so the correct error class exists in the handler but is unreachable for an unknown username.

In both causes the HTTP status matches (400) — only the error code, the x-amzn-errortype header and the message text diverge.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture cognito-idp-admin-auth-event-feedback-errors step task#2 — a Plus-tier pool with threat protection enabled, then AdminUpdateAuthEventFeedback(UserPoolId=<pool>, Username=<a username that was never created in the pool>, EventId=<any well-formed event id>, FeedbackValue=Valid). AWS: 400 NotAuthorizedException 'Incorrect username or password.'. Emulator: 400 UserNotFoundException 'User does not exist.'.
3. Repeat against cognito-idp-risk-configuration-and-auth-event-feedback step task#5 (same cluster) to confirm the fix covers both steps.
4. Cause 2: replay fixture cognito-idp-risk-configuration-and-auth-event-feedback step task#6 — UpdateAuthEventFeedback(UserPoolId=<pool>, Username=<a username that was never created>, EventId=<any well-formed event id>, FeedbackToken=<any token matching ^[A-Za-z0-9-_=.]+$>, FeedbackValue=Valid). AWS: 400 NotAuthorizedException 'Invalid session provided'. Emulator: 400 UserNotFoundException 'User does not exist.'.
5. Repeat against cognito-idp-update-auth-event-feedback-errors step task#2 (same cluster).
6. Cross-check the masking: issue the same two calls for a username that DOES exist in the pool but with an unknown EventId/FeedbackToken, and confirm the refusal a caller sees is indistinguishable from the unknown-username one as it is on AWS.
