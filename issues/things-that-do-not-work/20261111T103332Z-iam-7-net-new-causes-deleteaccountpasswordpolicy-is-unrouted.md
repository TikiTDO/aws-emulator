Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.account-password-policy-lifecycle iam.delete-account-password-policy iam.get-nonexistent-inline-policy-returns-nosuchentity iam.duplicate-upload-returns-specific-duplicate-errors iam.ssh-public-key-duplicate-rejected iam.ssh-public-key-duplicate-upload-rejected iam.ssh-public-key-server-computed-fingerprint-matches-key iam.upload-ssh-public-key-malformed-body-rejected iam.update-ssh-public-key-invalid-status-value-rejected iam.minted-identifier-prefixes-and-arn-paths iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|DeleteAccountPasswordPolicy|InvalidAction
Issue-Key: iam.duplicate-upload-returns-specific-duplicate-errors|run#11|AssertionError|assert msg_ssh == expected_ssh, f"expected message {expected_ssh!r}, got {msg_ssh!r}"
Issue-Key: iam.ssh-public-key-duplicate-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.ssh-public-key-duplicate-upload-rejected|run#11|AssertionError|assert message == "Duplicate SSH public key uploaded.", (
Issue-Key: iam.ssh-public-key-server-computed-fingerprint-matches-key|run#14|AssertionError|assert upload_resp["SSHPublicKey"]["Fingerprint"] == expected_fingerprint, (
Issue-Key: iam.upload-ssh-public-key-malformed-body-rejected|run#9|AssertionError|assert err.get("Message") == _MSG_UNRECOGNIZED, (err.get("Message"), _MSG_UNRECOGNIZED)
Issue-Key: iam.update-ssh-public-key-invalid-status-value-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.minted-identifier-prefixes-and-arn-paths|_assert_id#35|AssertionError|assert value.startswith(prefix), (
Issue-Key: iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected|run#22|AssertionError|assert err.get("Message") == violation, (
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# iam: 7 net-new causes — DeleteAccountPasswordPolicy is unrouted (InvalidAction), SSH-public-key refusal messages and fingerprint diverge, access key ids are EMULATOR-prefixed, and a password-policy violation is misworded

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAccountPasswordPolicy

## Description
Seven net-new root causes in the IAM control plane, all reached by behavioral checks in the `iam` suite section. All seven must be closed before this issue is submitted.

Cause 1 of 7: DeleteAccountPasswordPolicy is not routed at all.
AWS: Smithy iam#DeleteAccountPasswordPolicy — input Unit, output Unit, modelled errors LimitExceededException (awsQueryError LimitExceeded, 409), NoSuchEntityException (NoSuchEntity, 404), ServiceFailureException (ServiceFailure, 500). A successful call is HTTP 200 with an empty DeleteAccountPasswordPolicyResponse envelope; with no custom policy set for the account AWS answers 404 NoSuchEntity. The action is a first-class member of the IAM query API surface.
Emulator: the iam query-protocol action dispatcher has no entry for it, so every call is refused with error code `InvalidAction` and the message `unknown action: DeleteAccountPasswordPolicy`. This is NOT a scaffold stub — a stub answers 501 NotImplemented with "operation ... is scaffolded but not implemented in this emulator"; here the action name is simply unknown to the router.
Why: any client that sets and then clears the account password policy (every password-policy lifecycle, and every teardown that resets the account singleton) gets a protocol-level InvalidAction it cannot branch on; botocore raises it as a generic ClientError rather than the modelled NoSuchEntityException, and the account singleton can never be returned to its default state. Three unrelated checks die on it because they reset the policy in setup/teardown.
Fix: the iam service's query action dispatcher plus the operation's handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/) — the operation needs to be routed and to clear the account password-policy singleton, answering 404 NoSuchEntity when none is set.
Traces: iam.account-password-policy-lifecycle, iam.delete-account-password-policy, iam.get-nonexistent-inline-policy-returns-nosuchentity.

Cause 2 of 7: the DuplicateSSHPublicKey refusal message is not AWS's.
AWS: Smithy iam#UploadSSHPublicKey error DuplicateSSHPublicKeyException, awsQueryError code DuplicateSSHPublicKey, httpResponseCode 400. Recorded behavior: the message is the fixed text `Duplicate SSH public key uploaded.` — it has no variable parts.
Emulator: status 400 and code DuplicateSSHPublicKey are right, but the message is `The SSH public key is already associated with the specified IAM user.` — the model's documentation prose rather than the wire message.
Why: a client or operator tool that asserts on, logs or surfaces the refusal text sees a string AWS never sends for this error class.
Fix: the UploadSSHPublicKey validation path's error construction for DuplicateSSHPublicKeyException (message template only; code and status already match).
Note: the open issue 20261111T010549Z-iam-ssh-public-key-credentials-deleted-keys-stay.md describes the same message divergence from the WIRE corpus; it does not carry these behavioral check ids, so these checks are not covered by it.
Traces: iam.duplicate-upload-returns-specific-duplicate-errors, iam.ssh-public-key-duplicate-rejected, iam.ssh-public-key-duplicate-upload-rejected.

Cause 3 of 7: the SSH public key Fingerprint is not the MD5 digest of the key material.
AWS: Smithy iam#SSHPublicKey.Fingerprint -> publicKeyFingerprintType, required, @pattern ^[:\w]+$, documented "The MD5 message digest of the SSH public key". It is the MD5 over the base64-DECODED public-key blob (the second whitespace-separated field of the OpenSSH body), rendered as colon-separated lowercase hex byte pairs, and is therefore a pure function of the uploaded key material: the same body always yields the same digest, and it matches what `ssh-keygen -l -E md5` reports for that key. UploadSSHPublicKey and GetSSHPublicKey must both return that value.
Emulator: for a freshly generated RSA key the emulator returned a 16-byte colon-hex value that is not the MD5 of the decoded blob (one run: emulator `96:e9:7c:79:c6:7b:e4:8c:b6:ef:af:d5:2a:2c:21:49` vs the key's actual digest `ee:4e:1a:8f:17:aa:00:fa:9e:e4:7d:d2:69:a4:13:c1`; both hex strings are per-run values of that run's key — the requirement is the digest function, not either literal). Hypothesis: the digest is taken over the raw request string (type prefix/comment included) or over the base64 text rather than its decoded bytes.
Why: fingerprint comparison is the standard way to confirm which key was installed; a client that pins or compares the fingerprint never matches the key it uploaded.
Fix: UploadSSHPublicKey's (and GetSSHPublicKey's) fingerprint computation.
Traces: iam.ssh-public-key-server-computed-fingerprint-matches-key.

Cause 4 of 7: the UnrecognizedPublicKeyEncoding refusal message is not AWS's.
AWS: Smithy iam#UnrecognizedPublicKeyEncodingException, awsQueryError code UnrecognizedPublicKeyEncoding, httpResponseCode 400. For key material whose encoding format IAM does not recognize, AWS answers 400 with that code and the fixed message `Invalid public key uploaded.` (no variable parts).
Emulator: status 400 and code UnrecognizedPublicKeyEncoding are now right, but the message is `The public key encoding format is unsupported or unrecognized.` — again the model's documentation prose instead of the wire message.
Why: a client that asserts on or displays the refusal text gets a string AWS never sends; the code/message pair no longer agrees with AWS for this refusal class.
Fix: UploadSSHPublicKey's input-validation error construction for UnrecognizedPublicKeyEncodingException (message template only).
Traces: iam.upload-ssh-public-key-malformed-body-rejected.

Cause 5 of 7: the enum-constraint validation message renders the value set in the wrong order.
AWS: Smithy iam#UpdateSSHPublicKey.Status -> statusType, an enum. For a Status outside the set AWS answers 400 ValidationError whose message is `1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Expired, Active, Inactive]` — the fixed text is everything but the member path (`status`, taken from the request member) and the bracketed value set, which AWS renders in that order for statusType.
Emulator: every other byte of the message matches, but the set is rendered `[Active, Inactive, Expired]` — the model's declaration order rather than AWS's.
Why: a client or test that asserts on the validation message (the usual way to confirm which member failed and what it accepts) never matches AWS's text, and an operator reading it is given a different accepted-value list than AWS prints.
Fix: the shared query-protocol enum-constraint validation message formatter for iam (the ordering of the rendered value set), not the UpdateSSHPublicKey handler itself.
Traces: iam.update-ssh-public-key-invalid-status-value-rejected.

Cause 6 of 7: CreateAccessKey mints an access key id that is not an AWS IAM key id.
AWS: Smithy iam#AccessKey.AccessKeyId -> accessKeyIdType, @length 16-128, @pattern ^[\w]+$. AWS's IAM unique-identifier prefixes are documented and load-bearing: a long-term IAM user access key id is 20 characters and begins `AKIA` (a temporary STS key begins `ASIA`).
Emulator: CreateAccessKey answers a 20-character id prefixed `EMULATOR` (one run: `EMULATORAKZZTPZTELTK`; the suffix is per-run, the `EMULATOR` prefix is the defect).
Why: tooling that classifies a credential by its prefix — long-term vs temporary, IAM user vs role session, credential scanners and policy linters — misclassifies every key the emulator issues, and a client that validates an id against AWS's documented prefix rejects it.
Fix: CreateAccessKey's access-key-id minting (the id allocator for the IAM user access-key resource).
Related: 20261029T191504Z-sts-vended-credentials-and-policy-size-members-diverge.md covers STS's ASIA-prefixed temporary ids, a different minting site; it does not cover IAM's long-term AKIA ids.
Traces: iam.minted-identifier-prefixes-and-arn-paths.

Cause 7 of 7: the PasswordPolicyViolation message for a missing symbol is not AWS's.
AWS: Smithy iam#CreateLoginProfile error PasswordPolicyViolationException, awsQueryError code PasswordPolicyViolation, httpResponseCode 400. For a password that satisfies every other rule of the account password policy but contains no symbol, AWS's message is the fixed text `Password should have at least one symbol` (no variable parts).
Emulator: status 400 and code PasswordPolicyViolation are right, but the message is `Password should have at least one of the following symbols: ! @ # $ % ^ & * ( ) _ + - = [ ] { } | '` — a locally invented text that also enumerates a symbol set AWS does not put in this message.
Why: a client that maps the violation text to the specific policy rule it broke (the only way the API tells which of the five RequireX rules failed) cannot match AWS's string, and an operator is shown a different rule statement than AWS gives.
Fix: the CreateLoginProfile/UpdateLoginProfile password-policy validation's per-rule message templates.
Traces: iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1) against the emulator built from this checkout; iam section 360/464 passed.

Cause 1 — three checks die on the same unrouted action:
[error] iam.account-password-policy-lifecycle: botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the DeleteAccountPasswordPolicy operation: unknown action: DeleteAccountPasswordPolicy
[error] iam.delete-account-password-policy: identical InvalidAction / "unknown action: DeleteAccountPasswordPolicy"
[error] iam.get-nonexistent-inline-policy-returns-nosuchentity: identical InvalidAction / "unknown action: DeleteAccountPasswordPolicy"

Cause 2:
[mismatch] iam.ssh-public-key-duplicate-rejected: AssertionError: expected message 'Duplicate SSH public key uploaded.', got 'The SSH public key is already associated with the specified IAM user.'
[mismatch] iam.ssh-public-key-duplicate-upload-rejected: AssertionError: DuplicateSSHPublicKey message: 'The SSH public key is already associated with the specified IAM user.'
[mismatch] iam.duplicate-upload-returns-specific-duplicate-errors: AssertionError: expected message 'Duplicate SSH public key uploaded.', got 'The SSH public key is already associated with the specified IAM user.'

Cause 3:
[mismatch] iam.ssh-public-key-server-computed-fingerprint-matches-key: AssertionError: UploadSSHPublicKey fingerprint '96:e9:7c:79:c6:7b:e4:8c:b6:ef:af:d5:2a:2c:21:49' != expected 'ee:4e:1a:8f:17:aa:00:fa:9e:e4:7d:d2:69:a4:13:c1'. The check computes the expectation as ':'.join(md5(base64decode(body.split()[1])).hexdigest() pairs).

Cause 4:
[mismatch] iam.upload-ssh-public-key-malformed-body-rejected: AssertionError: ('The public key encoding format is unsupported or unrecognized.', 'Invalid public key uploaded.') — the assertion on Code == 'UnrecognizedPublicKeyEncoding' passed; only the message diverges.

Cause 5:
[mismatch] iam.update-ssh-public-key-invalid-status-value-rejected: AssertionError: expected message "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Expired, Active, Inactive]", got "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [Active, Inactive, Expired]"

Cause 6:
[mismatch] iam.minted-identifier-prefixes-and-arn-paths: AssertionError: access key unique id must carry the documented 'AKIA' prefix, got 'EMULATORAKZZTPZTELTK'

Cause 7:
[mismatch] iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected: AssertionError: CreateLoginProfile (no symbol) message: expected 'Password should have at least one symbol', got "Password should have at least one of the following symbols: ! @ # $ % ^ & * ( ) _ + - = [ ] { } | '"

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json):
- DeleteAccountPasswordPolicy: input Unit, output Unit, errors LimitExceeded(409), NoSuchEntity(404), ServiceFailure(500).
- UploadSSHPublicKey errors: DuplicateSSHPublicKey(400), InvalidPublicKey(400), LimitExceeded(409), NoSuchEntity(404), UnrecognizedPublicKeyEncoding(400); output SSHPublicKey.
- UpdateSSHPublicKey.Status -> statusType; statusType is an enum with values [Active, Inactive, Expired] (declaration order — AWS's validation message prints [Expired, Active, Inactive]).
- CreateAccessKey output AccessKey (required); accessKeyIdType @length 16-128 @pattern ^[\w]+$.
- CreateLoginProfile errors: EntityAlreadyExists(409), LimitExceeded(409), NoSuchEntity(404), PasswordPolicyViolation(400), ServiceFailure(500).

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and look at the `iam` section.
3. Cause 1: call iam DeleteAccountPasswordPolicy with any credentials — the response is error code InvalidAction, message "unknown action: DeleteAccountPasswordPolicy". Replay iam.account-password-policy-lifecycle, iam.delete-account-password-policy, iam.get-nonexistent-inline-policy-returns-nosuchentity.
4. Cause 2: CreateUser, UploadSSHPublicKey with a valid ssh-rsa body, then upload the identical body again; read Error.Message on the 400 DuplicateSSHPublicKey. Replay iam.ssh-public-key-duplicate-rejected, iam.ssh-public-key-duplicate-upload-rejected, iam.duplicate-upload-returns-specific-duplicate-errors.
5. Cause 3: UploadSSHPublicKey with a generated RSA OpenSSH public key and compare SSHPublicKey.Fingerprint with md5 over base64-decode(body.split()[1]) rendered as colon-separated hex pairs; GetSSHPublicKey must return the same value. Replay iam.ssh-public-key-server-computed-fingerprint-matches-key.
6. Cause 4: UploadSSHPublicKey with an unstructured body (no SSH/PEM framing) and read Error.Message on the 400 UnrecognizedPublicKeyEncoding. Replay iam.upload-ssh-public-key-malformed-body-rejected.
7. Cause 5: UpdateSSHPublicKey with Status set to a value outside the enum and read the ValidationError message's bracketed value set. Replay iam.update-ssh-public-key-invalid-status-value-rejected.
8. Cause 6: CreateUser then CreateAccessKey and inspect AccessKey.AccessKeyId's prefix. Replay iam.minted-identifier-prefixes-and-arn-paths.
9. Cause 7: UpdateAccountPasswordPolicy with RequireSymbols=true (plus the other Require* rules), then CreateLoginProfile with a password satisfying everything except the symbol rule; read Error.Message on the 400 PasswordPolicyViolation. Replay iam.iam-cred-password-policy-violation-and-role-session-change-password-rejected.
