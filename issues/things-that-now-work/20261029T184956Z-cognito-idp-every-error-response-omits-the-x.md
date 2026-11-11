Issue-Source: trace-sweep-divergence
Issue-Trace: cip-autherr-admin-reset-password-required
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-autherr-admin-reset-password-required cip-autherr-flow-not-enabled-user-password cip-autherr-signup-unconfirmed-user-not-confirmed cognito-idp-account-recovery-and-attribute-verification cognito-idp-self-service-signup-confirm-and-forgot-password cognito-idp-self-service-profile-view-update-and-attribute-verification cognito-idp-admin-user-auth-workflow cognito-idp-admin-user-core-lifecycle cognito-idp-admin-user-group-and-account-linking-lifecycle cognito-idp-list-users-in-pool cognito-idp-client-secret-limit-exceeded cognito-idp-user-pool-client-secret-errors cognito-idp-user-pool-client-secret-lifecycle cognito-idp-user-pool-client-secret-management cognito-idp-user-pool-client-secret-rotation cognito-idp-device-tracking-status-management cognito-idp-terms-document-lifecycle cognito-idp-terms-documents-crud cognito-idp-terms-documents-error-paths cognito-idp-terms-duplicate-create-error cognito-idp-terms-errors cognito-idp-user-pool-domain-delete-notfound-error cognito-idp-user-pool-domain-errors cognito-idp-risk-configuration-invalid-parameter-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|InitiateAuth|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ConfirmForgotPassword|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|VerifyUserAttribute|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminCreateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AddUserPoolClientSecret|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ConfirmDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateTerms|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateTerms|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteUserPoolDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetRiskConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: every error response omits the x-amzn-errortype header

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cognito Identity Provider is an awsJson1_1 service (Smithy: AWSCognitoIdentityProviderService, protocols [awsJson1_1]). Every error response it serves carries the response header `x-amzn-errortype`, whose value is the error shape's name followed by a single colon (e.g. `CodeMismatchException:`). The emulator never sets that header on ANY error response, on ANY operation. The clusters below are the recorded units that exercise it; they are ten separate error paths and a fixer must re-replay all ten.

AWS: for an awsJson1_1 error, AWS returns the JSON body `{"__type":"<ErrorShape>","message":"..."}` AND the response header `x-amzn-errortype`. On cognito-idp the recorded header value is the error shape's simple name with a trailing colon: fixed text is the colon, the variable part is the shape name taken from the error the handler raised. Smithy: all of these shapes are modelled error structures on the service (InvalidParameterException is even the service-common error).
Emulator: the header is absent on every error response (`$headers.x-amzn-errortype: missing ... got None`), for every operation and every error class.
Why: AWS SDKs and the CLI resolve the concrete exception type from `x-amzn-errortype` first and fall back to `__type` only when it is absent. Older/other-language SDKs that only read the header deserialize every cognito-idp refusal as a generic/unknown service error, so client code that branches on `UsernameExistsException` vs `InvalidParameterException` cannot distinguish them and retry/fallback logic silently stops working.
Fix: the service's error serializer (the awsJson1_1 error-response path shared by all cognito-idp handlers) must emit `x-amzn-errortype: <ErrorShape>:` alongside the `__type` body field. This is one serializer change that resolves all ten causes.

Cause 1 of 10: InitiateAuth refusals carry no x-amzn-errortype. Fixture cip-autherr-admin-reset-password-required, step task#7 (status 400 correct, header missing).
Cause 2 of 10: ConfirmForgotPassword CodeMismatchException carries no x-amzn-errortype (expected `CodeMismatchException:`). Fixture cognito-idp-account-recovery-and-attribute-verification, step task#6.
Cause 3 of 10: VerifyUserAttribute CodeMismatchException carries no x-amzn-errortype (expected `CodeMismatchException:`). Fixture cognito-idp-account-recovery-and-attribute-verification, step task#9.
Cause 4 of 10: AdminCreateUser UsernameExistsException carries no x-amzn-errortype (expected `UsernameExistsException:`). Fixture cognito-idp-admin-user-auth-workflow, step task#5.
Cause 5 of 10: AddUserPoolClientSecret LimitExceededException carries no x-amzn-errortype (expected `LimitExceededException:`). Fixture cognito-idp-client-secret-limit-exceeded, step task#6.
Cause 6 of 10: ConfirmDevice InvalidUserPoolConfigurationException carries no x-amzn-errortype (expected `InvalidUserPoolConfigurationException:`). Fixture cognito-idp-device-tracking-status-management, step task#6.
Cause 7 of 10: CreateTerms TermsExistsException carries no x-amzn-errortype (expected `TermsExistsException:`). Fixture cognito-idp-terms-document-lifecycle, step task#8.
Cause 8 of 10: UpdateTerms TermsExistsException carries no x-amzn-errortype (expected `TermsExistsException:`). Fixture cognito-idp-terms-documents-error-paths, step task#6.
Cause 9 of 10: DeleteUserPoolDomain InvalidParameterException carries no x-amzn-errortype (expected `InvalidParameterException:`). Fixture cognito-idp-user-pool-domain-delete-notfound-error, step task#2.
Cause 10 of 10: SetRiskConfiguration InvalidParameterException carries no x-amzn-errortype (expected `InvalidParameterException:`). Fixture cognito-idp-risk-configuration-invalid-parameter-error, step task#2.

The same omission appears in ~130 further recorded error steps across ForgetDevice, UpdateDeviceStatus, AdminForgetDevice, AdminUpdateDeviceStatus, ConfirmSignUp, SignUp, ListUserPools, GetUserAttributeVerificationCode, CreateGroup, CreateResourceServer and others; all of them are fixed by the same serializer change.

## Evidence
Expected (recorded AWS): response header `x-amzn-errortype` present on every error, value = error shape name + ':' (e.g. `CodeMismatchException:`, `UsernameExistsException:`, `LimitExceededException:`, `InvalidUserPoolConfigurationException:`, `TermsExistsException:`, `InvalidParameterException:`).
Actual (emulator): header absent — replay reports `$headers.x-amzn-errortype: missing (expected 'CodeMismatchException:', got None)` and equivalents on every error step. Response bodies otherwise contain `__type` and `message`.

## Steps to Reproduce
1. Replay fixture cip-autherr-admin-reset-password-required step task#7 against the emulator and inspect the HTTP response headers of the InitiateAuth 400.
2. Repeat for each fixture#step listed under the causes above (ConfirmForgotPassword, VerifyUserAttribute, AdminCreateUser, AddUserPoolClientSecret, ConfirmDevice, CreateTerms, UpdateTerms, DeleteUserPoolDomain, SetRiskConfiguration).
3. Observe that no response carries x-amzn-errortype, while the recorded AWS response for each carries `<ErrorShape>:`.
