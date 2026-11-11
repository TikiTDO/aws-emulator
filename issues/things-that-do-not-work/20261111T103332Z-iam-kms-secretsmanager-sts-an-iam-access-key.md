Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__secretsmanager__sts.deactivated-access-key-is-refused-by-sts-kms-and-secrets-manager
Issue-Key: iam__kms__secretsmanager__sts.deactivated-access-key-is-refused-by-sts-kms-and-secrets-manager|_first_refusals#15|AssertionError|raise AssertionError(
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# iam__kms__secretsmanager__sts: an IAM access key set Inactive by UpdateAccessKey is still accepted — sts, kms and secretsmanager keep serving requests signed with a deactivated key

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccessKey

## Description
Cause 1 of 1: the Status of a long-term IAM access key is not consulted during credential resolution, so deactivating a key does not revoke it.
AWS: UpdateAccessKey(UserName, AccessKeyId, Status='Inactive') makes the key unusable. A request signed with an Inactive key is refused at credential resolution — the key is not a valid credential, no principal is resolved and no policy is evaluated — and each service reports it in its own unknown-credential error with fixed message text: sts GetCallerIdentity -> `InvalidClientTokenId` / `The security token included in the request is invalid.`; kms ListKeys and secretsmanager ListSecrets -> `UnrecognizedClientException` / `The security token included in the request is invalid.`. The refusal takes effect for every service, within seconds of the status change.
Emulator: nothing is refused. All three calls signed with the Inactive key keep SUCCEEDING for the check's whole 45 s convergence budget — `still accepted by ['sts GetCallerIdentity', 'kms ListKeys', 'secretsmanager ListSecrets'], refused so far {}` — so the credential-resolution path treats an Inactive key exactly like an Active one.
Why: deactivation is the primary non-destructive way to revoke a credential (the documented first step of key rotation and of incident response, chosen precisely because it is reversible). An emulator in which it has no effect will let a client's rotation or break-glass procedure appear to succeed while the old key still holds full access, and no amount of waiting converges — a caller cannot verify revocation at all.
Fix: the shared SigV4 credential-resolution path — resolve the access key id to its IAM access-key record and reject the request unless Status is Active, raising each service's modelled unknown-credential error (sts InvalidClientTokenId, kms/secretsmanager UnrecognizedClientException) with AWS's fixed message. Related: 20261109T025528Z-ec2-instance-connect-sendserialconsolesshpublickey-error-response.md reports the same "Inactive key treated as valid" root cause reached through EC2 Instance Connect; it does not carry this check id, so this check is not covered by it.
Traces: iam__kms__secretsmanager__sts.deactivated-access-key-is-refused-by-sts-kms-and-secrets-manager.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### iam__kms__secretsmanager__sts` section (0/1 passed).

[mismatch] iam__kms__secretsmanager__sts.deactivated-access-key-is-refused-by-sts-kms-and-secrets-manager
  AssertionError: the deactivated key is refused by every service did not happen within 45s; still accepted by ['sts GetCallerIdentity', 'kms ListKeys', 'secretsmanager ListSecrets'], refused so far {}

The check creates a user and access key, confirms the key works, calls iam UpdateAccessKey with Status='Inactive', then polls the three calls for up to 45 s expecting ('InvalidClientTokenId', 'The security token included in the request is invalid.') from sts and ('UnrecognizedClientException', 'The security token included in the request is invalid.') from kms and secretsmanager. None of the three ever refused.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateUser and CreateAccessKey; confirm the key works with sts GetCallerIdentity, kms ListKeys and secretsmanager ListSecrets signed with it.
3. Call iam UpdateAccessKey(UserName, AccessKeyId, Status='Inactive').
4. Re-issue the same three signed calls. AWS refuses all three as unknown credentials (InvalidClientTokenId for sts, UnrecognizedClientException for kms and secretsmanager); the emulator keeps serving all three.
5. Replay iam__kms__secretsmanager__sts.deactivated-access-key-is-refused-by-sts-kms-and-secrets-manager.
