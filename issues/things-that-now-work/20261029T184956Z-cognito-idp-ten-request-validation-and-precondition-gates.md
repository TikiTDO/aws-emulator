Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-username-attributes-email-rejects-non-email-username
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-username-attributes-email-rejects-non-email-username cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-log-delivery-configuration cognito-idp-log-delivery-configuration-lifecycle cognito-idp-provisioned-limit-and-signing-certificate cognito-idp-provisioned-limit-round-trip cognito-idp-provisioned-limit-roundtrip cognito-idp-resource-server-and-oauth-scope-enforcement cognito-idp-resource-server-and-scoped-client cognito-idp-user-pool-client-domain-and-replica-configuration cognito-idp-user-pool-replica-error-paths cognito-idp-start-import-job-no-csv-precondition-error cip-cfg-admin-create-user-resend-confirmed-user cip-cfg-alias-email-duplicate-signup-and-alias-claim
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|SignUp|4xx->2xx|$.__type
Issue-Key: cognito-idp|SignUp|4xx->2xx|$.message
Issue-Key: cognito-idp|SignUp|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SignUp|4xx->2xx|status
Issue-Key: cognito-idp|UpdateUserAttributes|4xx->2xx|$.__type
Issue-Key: cognito-idp|UpdateUserAttributes|4xx->2xx|$.message
Issue-Key: cognito-idp|UpdateUserAttributes|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserAttributes|4xx->2xx|status
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->2xx|$.__type
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->2xx|$.message
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->2xx|status
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->2xx|$.__type
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->2xx|$.message
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateProvisionedLimit|4xx->2xx|status
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->2xx|$.__type
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->2xx|$.message
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->2xx|status
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$.__type
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$.message
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPoolClient|4xx->2xx|status
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->2xx|$.__type
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->2xx|$.message
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->2xx|status
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->2xx|$.__type
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->2xx|$.message
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->2xx|status
Issue-Key: cognito-idp|StartUserImportJob|4xx->2xx|$.__type
Issue-Key: cognito-idp|StartUserImportJob|4xx->2xx|$.message
Issue-Key: cognito-idp|StartUserImportJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|StartUserImportJob|4xx->2xx|status
Issue-Key: cognito-idp|AdminCreateUser|4xx->2xx|$.__type
Issue-Key: cognito-idp|AdminCreateUser|4xx->2xx|$.message
Issue-Key: cognito-idp|AdminCreateUser|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminCreateUser|4xx->2xx|status
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: ten request-validation and precondition gates accept calls AWS refuses

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Ten recorded requests that AWS refuses with a modeled exception are accepted by the emulator with HTTP 200 and a success body. Each cause names the operation, the rule, the error class and the exact message AWS returns.

AWS/Emulator/Why/Fix is per cause below; the shared pattern is a missing request-validation or state precondition in the operation handler, and the shared consequence is that a client written against the emulator never sees the refusal, so code paths that handle the exception are never exercised and a real deployment fails on the first call.

Cause 1 of 10: SignUp on a pool with UsernameAttributes=email accepts a non-email Username. AWS: 400 InvalidParameterException, message `Username should be an email.` (fixed text, no variable part). Emulator: 200. Fix: SignUp validation. Fixture cip-cfg-username-attributes-email-rejects-non-email-username, step task#3.
Cause 2 of 10: UpdateUserAttributes accepts a malformed phone_number. AWS: 400 InvalidParameterException, message `Invalid phone number format.`. Emulator: 200. Fix: UpdateUserAttributes attribute validation. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step task#10.
Cause 3 of 10: SetLogDeliveryConfiguration accepts a log destination it cannot deliver to. AWS: 400 InvalidParameterException, message `Invalid destination in the input.`. Emulator: 200. Fix: SetLogDeliveryConfiguration validation. Fixture cognito-idp-log-delivery-configuration, step task#3.
Cause 4 of 10: UpdateProvisionedLimit accepts a RequestedLimitValue above the account's quota ceiling. AWS: 400 ServiceQuotaExceededException (Smithy: ServiceQuotaExceededException is a modeled error of UpdateProvisionedLimit), message `Requested value <requested> exceeds Service Quota ceiling of <ceiling>` — fixed text with the requested value from the request and the ceiling from the account's quota state. Emulator: 200. Fix: UpdateProvisionedLimit quota check. Fixture cognito-idp-provisioned-limit-and-signing-certificate, step task#3.
Cause 5 of 10: CreateUserPoolClient accepts an AllowedOAuthScopes entry that no resource server defines. AWS: 400 ScopeDoesNotExistException, message `Invalid scope requested: <resourceServerIdentifier>/<scopeName>` — the identifier and scope come from the request. Emulator: 200. Fix: CreateUserPoolClient scope resolution against the pool's resource servers. Fixture cognito-idp-resource-server-and-oauth-scope-enforcement, step task#7.
Cause 6 of 10: UpdateUserPoolClient accepts token-validity values outside the modeled range. AWS: 400 InvalidParameterException, message `Invalid range for token validity.`. Emulator: 200. Smithy: AccessTokenValidity/IdTokenValidity @range 1..86400, RefreshTokenValidity @range 0..315360000, AuthSessionValidity @range 3..15. Fix: UpdateUserPoolClient validation. Fixture cognito-idp-user-pool-client-domain-and-replica-configuration, step task#5.
Cause 7 of 10: CreateUserPoolReplica accepts replication for a pool with no customer-managed KMS key. AWS: 400 InvalidParameterException, message `User pool needs a customer-managed KMS key for multi-region replication`. Emulator: 200. Fix: CreateUserPoolReplica precondition on the pool's KeyConfiguration. Fixture cognito-idp-user-pool-replica-error-paths, step task#3.
Cause 8 of 10: UpdateUserPoolReplica accepts a region in which no replica exists. AWS: 400 ResourceNotFoundException, message `Replica not found in the requested region`. Emulator: 200. Fix: UpdateUserPoolReplica existence check. Fixture cognito-idp-user-pool-replica-error-paths, step task#4.
Cause 9 of 10: StartUserImportJob starts a job whose CSV was never uploaded. AWS: 400 PreconditionNotMetException (a modeled error of StartUserImportJob), message `No csv file was uploaded for <jobId>` — fixed text with the job id from the request. Emulator: 200. Fix: StartUserImportJob precondition on the job's upload state. Fixture cognito-idp-start-import-job-no-csv-precondition-error, step task#4.
Cause 10 of 10: AdminCreateUser with MessageAction=RESEND against a user that is not FORCE_CHANGE_PASSWORD is accepted. AWS: 400 UnsupportedUserStateException, message `Resend not possible. <username> status is not FORCE_CHANGE_PASSWORD` — fixed text with the username from the request and the required status named literally. Emulator: 200. Fix: AdminCreateUser RESEND state gate. Fixture cip-cfg-admin-create-user-resend-confirmed-user, step task#5.

## Evidence
Each cause shows `status: expected 400, got 200` with the whole error envelope missing. Examples:
  SignUp (cip-cfg-username-attributes-email-rejects-non-email-username#task#3): $.__type expected 'InvalidParameterException' got None; $.message expected 'Username should be an email.' got None.
  UpdateProvisionedLimit (cognito-idp-provisioned-limit-and-signing-certificate#task#3): $.__type expected 'ServiceQuotaExceededException'; $.message expected 'Requested value 121 exceeds Service Quota ceiling of 120'.
  CreateUserPoolClient (cognito-idp-resource-server-and-oauth-scope-enforcement#task#7): $.__type expected 'ScopeDoesNotExistException'; $.message expected 'Invalid scope requested: <resource-server>/does-not-exist'.
  StartUserImportJob (cognito-idp-start-import-job-no-csv-precondition-error#task#4): $.__type expected 'PreconditionNotMetException'; $.message expected 'No csv file was uploaded for <jobId>'.
  AdminCreateUser (cip-cfg-admin-create-user-resend-confirmed-user#task#5): $.__type expected 'UnsupportedUserStateException'; $.message expected 'Resend not possible. <username> status is not FORCE_CHANGE_PASSWORD'.

## Steps to Reproduce
1. Replay cip-cfg-username-attributes-email-rejects-non-email-username step task#3 and observe a 200 SignUp where AWS answers 400 InvalidParameterException.
2. Replay each remaining fixture#step named in the causes above and compare the HTTP status and body with the recording.
