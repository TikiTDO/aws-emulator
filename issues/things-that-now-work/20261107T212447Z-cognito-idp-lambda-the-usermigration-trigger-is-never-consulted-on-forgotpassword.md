Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication
Issue-Key: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication|_retry_on_lambda_warmup#15|AssertionError|raise AssertionError(
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# The UserMigration trigger is never consulted on ForgotPassword — every unknown user is refused UserNotFoundException

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
Cause 1 of 1: a user pool with a UserMigration Lambda config refuses a legacy user that the trigger would migrate, instead of invoking the trigger and creating the user.

AWS: recorded behavior only. With LambdaConfig.UserMigration set, Cognito invokes the trigger when a username is not found in the pool — with `triggerSource` `UserMigration_Authentication` for a sign-in and `UserMigration_ForgotPassword` for a password reset — and, if the trigger answers with userAttributes and finalUserStatus, creates the user and continues the flow. A username the trigger declines is the only case that yields UserNotFoundException.

Emulator: the trigger is never consulted, so a known legacy user is refused like any unknown one. Replay detail: `AssertionError: the migration trigger never migrated the known legacy user within 90s, so it is refusing everyone rather than one username: UserNotFoundException('An error occurred (UserNotFoundException) when calling the ForgotPassword operation: Username/client id combination not found.')`.

Why: a client migrating an existing user directory into Cognito lazily cannot migrate anybody — every legacy user is permanently locked out, and because the refusal is identical for migratable and non-migratable usernames, the client cannot even detect that migration is not running. It also removes the triggerSource distinction a trigger needs to tell a reset from a sign-in.

Fix: the Cognito user-pool trigger dispatcher for the user-not-found path of ForgotPassword and InitiateAuth/AdminInitiateAuth — invoke LambdaConfig.UserMigration with the correct triggerSource and apply the returned userAttributes / finalUserStatus before deciding to refuse.

Traces: cognito-idp__lambda.user-migration-trigger-source-distinguishes-forgot-password-from-authentication.

## Evidence
Expected: ForgotPassword for a username unknown to the pool invokes the UserMigration trigger with triggerSource 'UserMigration_ForgotPassword'; when the trigger returns the user, the user is created and the reset proceeds.
Actual: UserNotFoundException with message "Username/client id combination not found." for a username the trigger would migrate — the trigger is never invoked.

## Steps to Reproduce
1. CreateUserPool with LambdaConfig.UserMigration = <fn> and grant Cognito lambda:InvokeFunction on it; have the trigger return userAttributes + finalUserStatus for one known legacy username.
2. Call ForgotPassword (and InitiateAuth USER_PASSWORD_AUTH) for that username.
3. The call is refused with UserNotFoundException and the trigger records no invocation.
