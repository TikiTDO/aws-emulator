Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-user-pool-mfa-config-reflects-latest-set
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-user-pool-mfa-config-reflects-latest-set
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|SetUserPoolMfaConfig|2xx->4xx|$.MfaConfiguration
Issue-Key: cognito-idp|SetUserPoolMfaConfig|2xx->4xx|$.SoftwareTokenMfaConfiguration
Issue-Key: cognito-idp|SetUserPoolMfaConfig|2xx->4xx|status
Issue-Key: cognito-idp|GetUserPoolMfaConfig|2xx->2xx|$.SoftwareTokenMfaConfiguration.Enabled
Issue-Commit: 5772abe834f24ca0a7d173ddfaf967982b48def5

# cognito-idp: user-pool MFA configuration cannot be turned back off — SetUserPoolMfaConfig refuses the write and GetUserPoolMfaConfig keeps reporting TOTP enabled

> _Note: this issue was identified against the code base at commit `5772abe834f24ca0a7d173ddfaf967982b48def5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetUserPoolMfaConfig

## Description
The user-pool MFA configuration is not fully re-writable: a subsequent SetUserPoolMfaConfig that keeps MfaConfiguration=OPTIONAL while de-activating the software-token (TOTP) method is refused with HTTP 400 instead of accepted, and because the write never lands the following read still reports TOTP as active. Two causes, one write-path and one read-path consequence; both must be closed.

Cause 1 of 2: SetUserPoolMfaConfig refuses a valid request that de-activates TOTP MFA while leaving MfaConfiguration=OPTIONAL.
  AWS: Smithy: SetUserPoolMfaConfig takes MfaConfiguration (UserPoolMfaType enum, values OFF | ON | OPTIONAL) and SoftwareTokenMfaConfiguration (SoftwareTokenMfaConfigType, whose member Enabled is a boolean with @default false, documented as "The activation state of TOTP MFA"). Nothing in the model couples the two: Enabled=false is the member's own default, and OPTIONAL is a legal MfaConfiguration value, so a call that sets MfaConfiguration=OPTIONAL together with SoftwareTokenMfaConfiguration={"Enabled": false} is a well-formed request. AWS accepts it and answers HTTP 200 echoing the configuration it has stored — MfaConfiguration "OPTIONAL" and SoftwareTokenMfaConfiguration {"Enabled": false} — the operation being the documented way to "set user pool multi-factor authentication (MFA) and passkey configuration". The only 400s modeled for this input are InvalidParameterException, InvalidSmsRoleAccessPolicyException, InvalidSmsRoleTrustRelationshipException and ConcurrentModificationException, none of which applies to a syntactically valid de-activation.
  Emulator: answers HTTP 400 and returns no body members at all — $.MfaConfiguration and $.SoftwareTokenMfaConfiguration are both absent where the 200 response should carry "OPTIONAL" and {"Enabled": false}. Hypothesis: the handler enforces an invented rule that MfaConfiguration=OPTIONAL requires at least one MFA method to remain enabled, and so rejects the request rather than storing it.
  Why: the pool's MFA posture becomes one-way. A client cannot disable TOTP (for example to switch to SMS- or email-only MFA, or to roll back a mis-applied setting) without tearing the pool down, and the refusal it gets is an error AWS never returns for that request, so the call is not retried or branched on correctly.
  Fix: SetUserPoolMfaConfig's handler validation and the user-pool MFA state it writes — accept any legal combination of MfaConfiguration and the per-method configuration blocks, store the supplied SoftwareTokenMfaConfiguration.Enabled value (including false), and echo the stored configuration in the 200 response.
  Traces: cognito-idp-user-pool-mfa-config-reflects-latest-set#task#4.

Cause 2 of 2: GetUserPoolMfaConfig reports SoftwareTokenMfaConfiguration.Enabled=true after the method was de-activated.
  AWS: Smithy: GetUserPoolMfaConfig output carries SoftwareTokenMfaConfiguration (SoftwareTokenMfaConfigType with member Enabled, @default false). The read is a plain projection of the pool's stored MFA configuration, so it must reflect the most recent successful SetUserPoolMfaConfig — after TOTP is de-activated it returns SoftwareTokenMfaConfiguration {"Enabled": false}.
  Emulator: returns $.SoftwareTokenMfaConfiguration.Enabled = true, i.e. the earlier activated state, because the de-activating write in cause 1 was rejected and the stored configuration was never updated.
  Why: a client that reads back the configuration to confirm a change sees the stale, opposite value, so a converge/verify loop never terminates and any logic that keys off TOTP availability (which challenge to offer, whether to prompt for authenticator enrolment) takes the wrong branch.
  Fix: the user-pool MFA state read by GetUserPoolMfaConfig — once cause 1 lands, the read must project the last stored configuration rather than a sticky "enabled" flag; it needs its own check that de-activation is observable, not just that the write returns 200.
  Traces: cognito-idp-user-pool-mfa-config-reflects-latest-set#task#5.

Not covered by existing open issues: "cognito-idp: response payloads diverge — read paths omit or misreport pool, client, domain and MFA state" (20261102T111936Z) and "cognito-idp: MFA/challenge state machine — MFA_SETUP never issued, MfaConfiguration=ON accepted with no method..." (20261029T182116Z) look adjacent, but the replay reports both of these steps as unowned (no owned_by), and neither of those issues covers a valid de-activating SetUserPoolMfaConfig being refused with 400.

## Evidence
Fixture cognito-idp-user-pool-mfa-config-reflects-latest-set.

Cause 1 — step task#4, SetUserPoolMfaConfig:
  expected HTTP 200 with $.MfaConfiguration = "OPTIONAL" and $.SoftwareTokenMfaConfiguration = {"Enabled": false}
  actual   HTTP 400; $.MfaConfiguration missing, $.SoftwareTokenMfaConfiguration missing
  divergence_key: service=cognito-idp|SetUserPoolMfaConfig|status=200->400|keys=$.MfaConfiguration,$.SoftwareTokenMfaConfiguration,status

Cause 2 — step task#5, GetUserPoolMfaConfig:
  expected $.SoftwareTokenMfaConfiguration.Enabled = false
  actual   $.SoftwareTokenMfaConfiguration.Enabled = true (status 200 both sides)
  divergence_key: service=cognito-idp|GetUserPoolMfaConfig|status=200->200|keys=$.SoftwareTokenMfaConfiguration.Enabled

Model evidence (com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json): SetUserPoolMfaConfig input {MfaConfiguration: UserPoolMfaType enum [OFF, ON, OPTIONAL], SoftwareTokenMfaConfiguration: SoftwareTokenMfaConfigType}, output echoes the same members; SoftwareTokenMfaConfigType.Enabled is a boolean with default false ("The activation state of TOTP MFA"); GetUserPoolMfaConfig output carries SoftwareTokenMfaConfiguration. No modeled error covers a well-formed de-activation request.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Replay fixture cognito-idp-user-pool-mfa-config-reflects-latest-set.
4. Cause 1: inspect step task#4 — SetUserPoolMfaConfig with MfaConfiguration=OPTIONAL and SoftwareTokenMfaConfiguration.Enabled=false (after an earlier call had activated TOTP) answers HTTP 400 instead of HTTP 200 echoing MfaConfiguration "OPTIONAL" and SoftwareTokenMfaConfiguration {"Enabled": false}.
5. Cause 2: inspect the next step task#5 — GetUserPoolMfaConfig on the same pool returns SoftwareTokenMfaConfiguration.Enabled=true instead of false.
6. After fixing, re-replay the fixture and confirm task#4 returns 200 with the stored configuration and task#5 reads back Enabled=false.
