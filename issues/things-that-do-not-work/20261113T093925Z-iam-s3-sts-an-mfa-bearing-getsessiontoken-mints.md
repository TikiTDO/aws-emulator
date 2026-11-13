Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session
Issue-Key: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session|run#56|AssertionError|assert session["Credentials"]["AccessKeyId"].startswith("ASIA"), (
Issue-Commit: 33c939d4ec001105a8b177394bd7a87b2127538e

# iam__s3__sts: an MFA-bearing GetSessionToken mints an AccessKeyId outside the ASIA class (prefix 'EMUL'), so the deny-unless-MFA check cannot grade its MFA session

> _Note: this issue was identified against the code base at commit `33c939d4ec001105a8b177394bd7a87b2127538e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSessionToken

## Description
Cause 1 of 1: the temporary credential returned by an MFA-authenticated GetSessionToken carries an AccessKeyId that is not in the ASIA class, so the deny-unless-MFA scenario (an identity policy that denies everything unless aws:MultiFactorAuthPresent is true, exercised with a plain session and with an MFA-bearing session) stops before it can compare the two sessions' access.

AWS: Smithy (com.amazonaws.sts#GetSessionToken) models Credentials$AccessKeyId as accessKeyIdType, @required, @length(16..128), @pattern ^[\w]*$ and documents it as "The access key ID that identifies the temporary security credentials" — the model constrains the shape only. The class token is AWS's documented identifier prefix rule (AWS IAM User Guide, "IAM identifiers" → unique identifier prefixes): ASIA = temporary (AWS STS) access key id, AKIA = IAM user access key id. The requirement is the leading four-character class token `ASIA`; the remaining characters are a per-mint random id and are NOT part of the requirement.
Emulator: the MFA-bearing GetSessionToken mint answers an AccessKeyId in an `EMULATOR…` class. Replay: `AssertionError: an MFA-bearing GetSessionToken must return a temporary key in the ASIA class; got prefix 'EMUL'`. The same class is used for long-term IAM user keys, so a short-lived MFA session key and a long-term user key are indistinguishable by prefix.
Why: clients branch on the key-id class — whether a credential needs an accompanying session token, whether it is rotatable, whether it is safe to persist, and (for GetAccessKeyInfo) that the owning account is encoded in the key id itself. Any code path that tests for ASIA concludes no STS session was issued and treats an MFA-scoped session key as a long-term user key. Here it additionally costs the whole scenario: the check cannot proceed to the aws:MultiFactorAuthPresent comparison (plain session denied / MFA session allowed on S3), so the MFA-conditioned deny is left ungraded.
Fix: the temporary-credential key-id minting the STS session minter asks the shared credential store for — `crate::state::mint_session` in services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs delegating to the emulator's CredentialStore, reached from services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_session_token.rs. The session mint must request an ASIA-class key id while IAM long-term keys keep AKIA.
Related but NOT covering this check: open issue 20261027T130805Z-sts-5-net-new-behavioral-root-causes-temporary.md describes the same ASIA-class defect as its Cause 1, but only for checks of the `sts` group (assume-role-external-id-success-with-correct-id, get-access-key-info-*, get-caller-identity-userid-matches-created-role-id, get-federation-token-policy-caps-caller-permissions, assume-role-packed-policy-too-large-rejected); it does not carry this check's replay key, and this is the MFA-authenticated GetSessionToken mint in the iam__s3__sts group.
Traces: iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session.

## Evidence
Check iam__s3__sts.deny-unless-mfa-present-blocks-plain-session-but-allows-mfa-session [mismatch]:
  File "check_iam__s3__sts_deny_unless_mfa_present_blocks_plain_session_but_allows_mfa_session.py", line 288, in run
    assert session["Credentials"]["AccessKeyId"].startswith("ASIA")
  AssertionError: an MFA-bearing GetSessionToken must return a temporary key in the ASIA class; got prefix 'EMUL'

Expected (AWS): Credentials.AccessKeyId of an MFA-authenticated sts:GetSessionToken begins with the class token `ASIA` (the rest is a per-mint id and is not a requirement).
Actual (emulator): it begins with `EMUL` (an `EMULATOR…` class), the same class used for long-term IAM user keys.

Legs that passed before the divergence, so the mint itself and the MFA enrolment are reached: the user, its virtual MFA device and its enabled TOTP enrolment are created, the bootstrap NotAction exemption list is attached, and GetSessionToken with a correctly computed TokenCode SUCCEEDS and returns Credentials — only the key-id class diverges, and the subsequent comparison (the plain session denied vs the MFA session allowed under a Deny-unless-aws:MultiFactorAuthPresent policy) is never reached.

## Steps to Reproduce
1. iam.create_user(UserName=<u>); iam.create_virtual_mfa_device(VirtualMFADeviceName=<d>); iam.enable_mfa_device(UserName=<u>, SerialNumber=<serial>, AuthenticationCode1/2 = two consecutive real TOTP codes from Base32StringSeed); iam.create_access_key(UserName=<u>).
2. With that user's long-term key: sts.get_session_token(SerialNumber=<serial>, TokenCode=<current TOTP>). The call succeeds.
3. Read resp["Credentials"]["AccessKeyId"]. Expect it to start with `ASIA`; observe it starting with `EMULATOR`.
