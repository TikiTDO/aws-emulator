Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-create-import-job-malformed-role-arn-error
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-create-import-job-malformed-role-arn-error cognito-idp-risk-configuration-without-addon cognito-idp-admin-auth-event-feedback-addon-not-enabled cognito-idp-auth-challenge-and-token-refresh cognito-idp-invalid-inputs-and-missing-resources cognito-idp-client-credentials-m2m-flow cognito-idp-m2m-client-credentials-token cognito-idp-m2m-get-client-token cognito-idp-provisioned-limit-errors cognito-idp-mfa-and-log-delivery-configuration cognito-idp-webauthn-credential-management cognito-idp-webauthn-credentials-management cognito-idp-webauthn-error-paths-and-listing
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateUserImportJob|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateUserImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeRiskConfiguration|4xx->4xx|$.message
Issue-Key: cognito-idp|DescribeRiskConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminUpdateAuthEventFeedback|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminUpdateAuthEventFeedback|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|InitiateAuth|4xx->4xx|$.message
Issue-Key: cognito-idp|InitiateAuth|4xx->4xx|status
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->4xx|$.message
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetTokensFromRefreshToken|4xx->4xx|status
Issue-Key: cognito-idp|GetClientToken|4xx->4xx|status
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->4xx|$.message
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|StartWebAuthnRegistration|4xx->4xx|$.message
Issue-Key: cognito-idp|StartWebAuthnRegistration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteWebAuthnCredential|4xx->4xx|$.message
Issue-Key: cognito-idp|DeleteWebAuthnCredential|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteWebAuthnCredential|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Related: 20261029T184956Z-cognito-idp-seven-refusals-are-raised-as-the.md
Issue-Related: 20261029T184956Z-cognito-idp-ten-not-found-refusal-messages-diverge.md
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: validation and auth refusal message rendering diverges from AWS

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserImportJob

## Description
Ten recorded causes in how refusal messages are RENDERED: the constraint-violation template leaks regex anchors, enumerates enum values in the wrong order, and several hand-written texts differ from AWS's by punctuation or wording.

Cause 1 of 10: the `1 validation error detected: ...` template renders a @pattern constraint with its `^`/`$` anchors. AWS prints the pattern WITHOUT the anchors: `Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>`. The fixed text is the whole sentence; the slots are the submitted value, the member name, and the modeled pattern stripped of its anchors. Fix: the constraint-message renderer shared by all validated members. Fixture cognito-idp-create-import-job-malformed-role-arn-error, step task#2 (CreateUserImportJob, cloudWatchLogsRoleArn).
Cause 2 of 10: the same anchor leak on a userPoolId pattern. Smithy: UserPoolIdType @pattern `^[\\w-]+_[0-9a-zA-Z]+$`; AWS prints `[\\w-]+_[0-9a-zA-Z]+`. Fixture cognito-idp-risk-configuration-without-addon, step task#6 (DescribeRiskConfiguration).
Cause 3 of 10: the enum branch of the same template lists the value set in the wrong order. AWS enumerates the enum's members in the order the model declares them — here `[Invalid, Valid]`; the emulator prints `[Valid, Invalid]`. Fix: the enum-constraint renderer must iterate the modeled member order, not a re-sorted set. Fixture cognito-idp-admin-auth-event-feedback-addon-not-enabled, step task#5 (AdminUpdateAuthEventFeedback; the same step also shows the anchor/ordering template handling two violations in one message).
Cause 4 of 10: InitiateAuth with an unusable refresh token. AWS: NotAuthorizedException, message `Invalid Refresh Token` — no trailing period. Emulator: `Invalid Refresh Token.`. Fixture cognito-idp-auth-challenge-and-token-refresh, step task#9.
Cause 5 of 10: the same trailing-period divergence on GetTokensFromRefreshToken. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#7.
Cause 6 of 10: GetClientToken with a wrong client secret. AWS: NotAuthorizedException, message `Invalid client or secret` — deliberately ambiguous between client id and secret. Emulator: `Invalid client secret.`, which discloses which half was wrong. Fixture cognito-idp-client-credentials-m2m-flow, step task#5 (4 recorded steps).
Cause 7 of 10: UpdateProvisionedLimit above the ceiling. AWS: ServiceQuotaExceededException, message `Requested value <requested> exceeds Service Quota ceiling of <ceiling>` — fixed text with the requested value from the request and the ceiling from the account's quota state. Emulator: a multi-sentence paragraph (`The requested limit value <v> exceeds your account's maximum limit of <n> requests per second for this limit. To increase your limit, submit a Service Quotas increase request.`) and a different ceiling. Fixture cognito-idp-provisioned-limit-errors, step task#2.
Cause 8 of 10: SetLogDeliveryConfiguration with a log level the event source does not support. AWS: InvalidParameterException, message `Invalid destination in the input.`. Emulator: `Request validation Failed. Following event sources in request have an unsupported log level: [<eventSource>].`. Fixture cognito-idp-mfa-and-log-delivery-configuration, step task#6.
Cause 9 of 10: StartWebAuthnRegistration / CompleteWebAuthnRegistration on a pool without WebAuthn. AWS: WebAuthnNotEnabledException, message `WebAuthn not enabled for this pool.`. Emulator: `WebAuthn is not enabled for this user pool.`. Fixture cognito-idp-webauthn-credential-management, step task#7 (3 recorded steps; the CompleteWebAuthnRegistration cluster at cognito-idp-webauthn-credential-management#task#8 shows the same text).
Cause 10 of 10: DeleteWebAuthnCredential for an unknown credential. AWS: ResourceNotFoundException, message `The credential does not exist`. Emulator: `Credential does not exist.`. Fixture cognito-idp-webauthn-credential-management, step task#9 (3 recorded steps).

Related open issues that do not carry these replay keys: 20261029T182116Z-cognito-idp-input-validation-refusals-carry-the-wrong.md and 20261029T182116Z-cognito-idp-webauthn-error-messages-are-emulator-prose.md.

## Evidence
CreateUserImportJob (cognito-idp-create-import-job-malformed-role-arn-error#task#2): expected '... Member must satisfy regular expression pattern: arn:[\\w+=/,.@-]+:...', got the same text with the pattern wrapped in ^...$.
DescribeRiskConfiguration (cognito-idp-risk-configuration-without-addon#task#6): expected pattern '[\\w-]+_[0-9a-zA-Z]+', got '^[\\w-]+_[0-9a-zA-Z]+$'.
AdminUpdateAuthEventFeedback (cognito-idp-admin-auth-event-feedback-addon-not-enabled#task#5): expected 'Member must satisfy enum value set: [Invalid, Valid]', got '[Valid, Invalid]'.
InitiateAuth (cognito-idp-auth-challenge-and-token-refresh#task#9): expected 'Invalid Refresh Token', got 'Invalid Refresh Token.'
GetClientToken (cognito-idp-client-credentials-m2m-flow#task#5): expected 'Invalid client or secret', got 'Invalid client secret.'
UpdateProvisionedLimit (cognito-idp-provisioned-limit-errors#task#2): expected 'Requested value 2000000000 exceeds Service Quota ceiling of 50', got the multi-sentence paragraph with a 10000 ceiling.
SetLogDeliveryConfiguration (cognito-idp-mfa-and-log-delivery-configuration#task#6): expected 'Invalid destination in the input.', got 'Request validation Failed. Following event sources in request have an unsupported log level: [userNotification].'
StartWebAuthnRegistration (cognito-idp-webauthn-credential-management#task#7): expected 'WebAuthn not enabled for this pool.', got 'WebAuthn is not enabled for this user pool.'
DeleteWebAuthnCredential (cognito-idp-webauthn-credential-management#task#9): expected 'The credential does not exist', got 'Credential does not exist.'

## Steps to Reproduce
1. Replay cognito-idp-create-import-job-malformed-role-arn-error step task#2 and cognito-idp-risk-configuration-without-addon step task#6; compare the rendered regex with the recording (anchors must be absent).
2. Replay cognito-idp-admin-auth-event-feedback-addon-not-enabled step task#5 and compare the enum value-set order.
3. Replay the remaining fixture#step pairs named in the causes and diff $.message character-for-character.
