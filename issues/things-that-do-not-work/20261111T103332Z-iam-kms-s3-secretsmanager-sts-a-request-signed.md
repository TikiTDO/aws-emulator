Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__s3__secretsmanager__sts.deleted-access-key-is-refused-by-sts-s3-kms-and-secrets-manager
Issue-Key: iam__kms__s3__secretsmanager__sts.deleted-access-key-is-refused-by-sts-s3-kms-and-secrets-manager|run#16|AssertionError|assert refused == expected, (
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# iam__kms__s3__secretsmanager__sts: a request signed with a DELETED IAM access key is refused by s3/kms/secretsmanager as AccessDenied "Sign the request with the emulator credentials" instead of each service's unknown-credential error

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuckets

## Description
Cause 1 of 1: a credential the IAM store no longer holds is reported as an authorization failure with an emulator-specific message, instead of as an unknown credential in each service's own error vocabulary.
AWS: after DeleteAccessKey, a request signed with that key id is refused at credential resolution — the key does not exist, so no principal is resolved and no policy is evaluated. Each service reports it in its own protocol's unknown-credential error, with fixed message text and no variable parts:
  - sts GetCallerIdentity -> `InvalidClientTokenId` / `The security token included in the request is invalid.`
  - s3 ListBuckets -> `InvalidAccessKeyId` / `The AWS Access Key Id you provided does not exist in our records.`
  - kms ListKeys -> `UnrecognizedClientException` / `The security token included in the request is invalid.`
  - secretsmanager ListSecrets -> `UnrecognizedClientException` / `The security token included in the request is invalid.`
Emulator: STS is correct, but s3, kms and secretsmanager all answer `AccessDenied` with the message `Sign the request with the emulator credentials` — an emulator-specific string that is not an AWS error message, carrying a code that means "the principal is known but not permitted" rather than "this key does not exist".
Why: a client cannot distinguish a revoked/rotated credential from an insufficient policy, which are the two cases it must handle differently (re-fetch credentials vs fix the policy). SDK credential-refresh and retry logic keys on InvalidAccessKeyId / UnrecognizedClientException / InvalidClientTokenId and never fires, so a caller whose key was deleted retries forever against an AccessDenied it reads as a permissions problem. The message also leaks emulator-specific instructions into a response an AWS client parses.
Fix: the shared SigV4 credential-resolution path in front of the s3, kms and secretsmanager handlers — when the access key id resolves to no IAM credential, raise each service's modelled unknown-credential error (s3 InvalidAccessKeyId; kms and secretsmanager UnrecognizedClientException) with AWS's fixed message, instead of falling through to the generic AccessDenied "sign with the emulator credentials" refusal.
Traces: iam__kms__s3__secretsmanager__sts.deleted-access-key-is-refused-by-sts-s3-kms-and-secrets-manager.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### iam__kms__s3__secretsmanager__sts` section (0/1 passed).

[mismatch] iam__kms__s3__secretsmanager__sts.deleted-access-key-is-refused-by-sts-s3-kms-and-secrets-manager
  AssertionError: the key is deleted, so each call signed with it must be refused as an unknown credential with exactly
  {'sts GetCallerIdentity': ('InvalidClientTokenId', 'The security token included in the request is invalid.'),
   's3 ListBuckets': ('InvalidAccessKeyId', 'The AWS Access Key Id you provided does not exist in our records.'),
   'kms ListKeys': ('UnrecognizedClientException', 'The security token included in the request is invalid.'),
   'secretsmanager ListSecrets': ('UnrecognizedClientException', 'The security token included in the request is invalid.')};
  got
  {'sts GetCallerIdentity': ('InvalidClientTokenId', 'The security token included in the request is invalid.'),
   's3 ListBuckets': ('AccessDenied', 'Sign the request with the emulator credentials'),
   'kms ListKeys': ('AccessDenied', 'Sign the request with the emulator credentials'),
   'secretsmanager ListSecrets': ('AccessDenied', 'Sign the request with the emulator credentials')}

STS matched; the three other services share one wrong (code, message) pair.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateUser, CreateAccessKey, and confirm the key works (e.g. sts GetCallerIdentity signed with it).
3. DeleteAccessKey for that key id.
4. Sign and send, with the deleted key: sts GetCallerIdentity, s3 ListBuckets, kms ListKeys, secretsmanager ListSecrets, and read (Error.Code, Error.Message) from each.
5. AWS answers InvalidClientTokenId / InvalidAccessKeyId / UnrecognizedClientException / UnrecognizedClientException with the messages above; the emulator answers AccessDenied 'Sign the request with the emulator credentials' for the last three.
6. Replay iam__kms__s3__secretsmanager__sts.deleted-access-key-is-refused-by-sts-s3-kms-and-secrets-manager.
