Issue-Source: trace-sweep-divergence
Issue-Trace: iam-ssh-public-key-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-ssh-public-key-lifecycle iam-cred-get-ssh-public-key-ssh-and-pem-encodings iam-cred-ssh-key-rsa-1024-invalid-public-key iam-credential-conflict-and-malformed-errors iam-ssh-public-key-invalid-material iam-ssh-public-key-upload-and-status-update-errors iam-ssh-public-key-error-paths
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListSSHPublicKeys|2xx->2xx|$.ListSSHPublicKeysResponse.ListSSHPublicKeysResult.SSHPublicKeys
Issue-Key: iam|UploadSSHPublicKey|2xx->2xx|$.UploadSSHPublicKeyResponse.UploadSSHPublicKeyResult.SSHPublicKey.Fingerprint
Issue-Key: iam|UploadSSHPublicKey|2xx->2xx|$.UploadSSHPublicKeyResponse.UploadSSHPublicKeyResult.SSHPublicKey.SSHPublicKeyId
Issue-Key: iam|UploadSSHPublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|UploadSSHPublicKey|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d

# iam SSH public key credentials: deleted keys stay listed, ids and fingerprints are not AWS's, and upload refusals are misworded

> _Note: this issue was identified against the code base at commit `7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadSSHPublicKey

## Description
Six net-new causes in the IAM SSH-public-key credential surface (UploadSSHPublicKey / ListSSHPublicKeys / DeleteSSHPublicKey). They share one fault: the SSH-public-key credential is not modelled as AWS models it — its identifier and fingerprint are not derived the way AWS derives them, its deletion does not reach the user's key collection, and its upload refusals carry codes and messages that are not AWS's. All six must be closed before this issue is submitted.

Cause 1 of 6: a deleted SSH public key is still returned by ListSSHPublicKeys.
AWS: Smithy iam#DeleteSSHPublicKey(UserName required, SSHPublicKeyId required) removes the key from the IAM user; a following iam#ListSSHPublicKeys for that user returns SSHPublicKeys as an empty list (output member SSHPublicKeys -> SSHPublicKeyListType). The only modelled error on either operation is NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404). Recorded behavior: after the delete step the list response carries no SSHPublicKeys members at all.
Emulator: ListSSHPublicKeys keeps returning one member for the user (UserName=<the probe user>, SSHPublicKeyId=APKABGXEQES5NRCCJJWAN, Status=Active, UploadDate=<upload time>) and the harness polled it 30 times without the list ever draining.
Why: a client that deletes a key and then enumerates the user's keys sees the key forever — credential rotation and teardown loops never converge, and a caller that re-uploads the same material gets a spurious duplicate refusal because the stale key is still associated.
Fix: DeleteSSHPublicKey's handler/state mutation for the user's SSH-public-key collection (the delete never removes the entry the list projection reads).
Traces: iam-ssh-public-key-lifecycle#task#9 (ListSSHPublicKeys, expected empty SSHPublicKeys, got 1 member).

Cause 2 of 6: UploadSSHPublicKey issues a single constant SSHPublicKeyId, and the Fingerprint is not the MD5 digest AWS computes.
AWS: Smithy iam#SSHPublicKey.SSHPublicKeyId -> publicKeyIdType, required, @length 20-128, @pattern ^[\w]+$, documented as "The unique identifier for the SSH public key" — AWS mints a fresh unique id (APKA-prefixed) per uploaded key. iam#SSHPublicKey.Fingerprint -> publicKeyFingerprintType, required, @pattern ^[:\w]+$, documented as "The MD5 message digest of the SSH public key", i.e. colon-separated hex over the key's binary public-key blob, so the same key material always yields the same digest and different material yields different digests.
Emulator: the recorded expectation for SSHPublicKeyId is a shape/uniqueness assertion ('<shaped-not-asserted>') and the emulator's value fails it: every upload, in every fixture and for every user, is answered with the one literal id APKABGXEQES5NRCCJJWAN. The Fingerprint returned for the recorded key material differs from the digest AWS returns for that same material (recording: 89:f4:36:e3:6c:f9:47:9a:31:73:78:67:ac:08:6f:3e; emulator: b0:a2:22:73:b7:33:b9:39:57:be:27:a3:8f:f2:5f:2d) — the emulator is digesting something other than the decoded public-key blob (Hypothesis: the raw request string including the key type prefix/comment, or the base64 text rather than its decoded bytes).
Why: a client that uploads two keys, or keys for two users, cannot tell them apart — GetSSHPublicKey/UpdateSSHPublicKey/DeleteSSHPublicKey by id all address the same record, and a second upload collides. A client that pins or compares the fingerprint (the standard way to confirm which key was installed, e.g. against `ssh-keygen -l`) never matches.
Fix: UploadSSHPublicKey's handler/state — id minting must allocate a unique per-key identifier, and fingerprint computation must MD5 the decoded public-key blob.
Traces: iam-cred-get-ssh-public-key-ssh-and-pem-encodings#task#2 (SSHPublicKeyId and Fingerprint both diverge; also iam-cred-ssh-key-rsa-1024-invalid-public-key#task#2).

Cause 3 of 6: the DuplicateSSHPublicKey refusal message is not AWS's.
AWS: Smithy iam#DuplicateSSHPublicKeyException, awsQueryError code DuplicateSSHPublicKey, httpResponseCode 400. Recorded response message, fixed text with no variable parts: "Duplicate SSH public key uploaded."
Emulator: status 400 and code DuplicateSSHPublicKey are right, but the message is "The SSH public key is already associated with the specified IAM user." — the model's documentation prose, not the wire message.
Why: a client or test that asserts on the refusal text (or surfaces it to an operator) sees a string AWS never sends for this error class.
Fix: the UploadSSHPublicKey validation path's error construction for DuplicateSSHPublicKeyException (message template only; the code and status are already correct).
Traces: iam-cred-get-ssh-public-key-ssh-and-pem-encodings#task#5 (cluster of 6 steps, also iam-cred-ssh-key-rsa-1024-invalid-public-key#task#3, iam-credential-conflict-and-malformed-errors#task#4 and #task#5, iam-ssh-public-key-invalid-material#task#1, iam-ssh-public-key-upload-and-status-update-errors#task#2).

Cause 4 of 6: a fingerprint divergence on a second key material (same root as cause 2, separate replay unit).
AWS: as cause 2 — Fingerprint is the MD5 message digest of the uploaded SSH public key, so it is a pure function of the key material.
Emulator: for this fixture's key the emulator answers a different digest than AWS does (recording 9e:2d:04:b9:e5:38:85:11:4e:84:55:32:6b:8e:12:2c vs emulator 71:5f:0e:1c:e5:f1:e1:c0:eb:43:1f:95:ee:ef:85:fa), confirming the digest input — not one key's value — is wrong.
Why: same as cause 2: fingerprint-based identification of an installed key never matches AWS.
Fix: UploadSSHPublicKey fingerprint computation (shared with cause 2); this trace is the second replay unit that must go green.
Traces: iam-credential-conflict-and-malformed-errors#task#3.

Cause 5 of 6: an unsupported public-key encoding is refused with the wrong error code and message.
AWS: Smithy iam#UploadSSHPublicKey models both InvalidPublicKeyException (awsQueryError InvalidPublicKey, 400) and UnrecognizedPublicKeyEncodingException (awsQueryError UnrecognizedPublicKeyEncoding, 400, documented "The request was rejected because the public key encoding format is unsupported or unrecognized"). For key material whose *encoding format* is not one AWS recognizes, AWS answers 400 with Code UnrecognizedPublicKeyEncoding and the fixed message "Invalid public key uploaded." (no variable parts).
Emulator: 400 with Code InvalidPublicKey and message "The public key is malformed or otherwise invalid." — it collapses the two modelled refusals onto the malformed-material one and invents the message text.
Why: a client that branches on the error code (retry with a re-encoded key vs reject the key outright) takes the wrong branch, and SDK error-shape dispatch never produces UnrecognizedPublicKeyEncodingException at all.
Fix: UploadSSHPublicKey's input validation — distinguish unrecognized encoding from malformed material and raise the corresponding modelled error with AWS's message.
Traces: iam-ssh-public-key-error-paths#task#3.

Cause 6 of 6: the constant SSHPublicKeyId again (same root as cause 2, separate replay unit).
AWS: as cause 2 — SSHPublicKeyId is a per-key unique identifier matching ^[\w]+$ with @length 20-128.
Emulator: this independent lifecycle fixture is answered with the identical literal id APKABGXEQES5NRCCJJWAN, which is what proves the value is a hard-coded constant rather than a minted id.
Why: as cause 2 — no two uploaded keys are distinguishable.
Fix: UploadSSHPublicKey id minting (shared with cause 2); this trace is the second replay unit that must go green.
Traces: iam-ssh-public-key-lifecycle#task#2.

No open issue owns any of these six clusters (the replay reports owned_by empty and stub_path empty for all six).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false):

1) iam-ssh-public-key-lifecycle#task#9 ListSSHPublicKeys, status 200 vs 200: $.ListSSHPublicKeysResponse.ListSSHPublicKeysResult.SSHPublicKeys: type_mismatch (expected '', got {'member': {'UserName': 'sim2real-probe-iam-sshuser-lifecycle-31ca9d81', 'SSHPublicKeyId': 'APKABGXEQES5NRCCJJWAN', 'Status': 'Active', 'UploadDate': '2026-11-11T01:01:16.929Z'}}); poll: 1 item(s), not 0 after 30 attempt(s).

2) iam-cred-get-ssh-public-key-ssh-and-pem-encodings#task#2 UploadSSHPublicKey, 200 vs 200: SSHPublicKey.SSHPublicKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'APKABGXEQES5NRCCJJWAN'); SSHPublicKey.Fingerprint: value_mismatch (expected '89:f4:36:e3:6c:f9:47:9a:31:73:78:67:ac:08:6f:3e', got 'b0:a2:22:73:b7:33:b9:39:57:be:27:a3:8f:f2:5f:2d'). affected_count=2.

3) iam-cred-get-ssh-public-key-ssh-and-pem-encodings#task#5 UploadSSHPublicKey, 400 vs 400: $.ErrorResponse.Error.Message: value_mismatch (expected 'Duplicate SSH public key uploaded.', got 'The SSH public key is already associated with the specified IAM user.'). affected_count=6.

4) iam-credential-conflict-and-malformed-errors#task#3 UploadSSHPublicKey, 200 vs 200: SSHPublicKey.Fingerprint: value_mismatch (expected '9e:2d:04:b9:e5:38:85:11:4e:84:55:32:6b:8e:12:2c', got '71:5f:0e:1c:e5:f1:e1:c0:eb:43:1f:95:ee:ef:85:fa').

5) iam-ssh-public-key-error-paths#task#3 UploadSSHPublicKey, 400 vs 400: $.ErrorResponse.Error.Code: value_mismatch (expected 'UnrecognizedPublicKeyEncoding', got 'InvalidPublicKey'); $.ErrorResponse.Error.Message: value_mismatch (expected 'Invalid public key uploaded.', got 'The public key is malformed or otherwise invalid.').

6) iam-ssh-public-key-lifecycle#task#2 UploadSSHPublicKey, 200 vs 200: SSHPublicKey.SSHPublicKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'APKABGXEQES5NRCCJJWAN') — the same literal id as (2), from a different fixture and a different user.

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): UploadSSHPublicKey errors = DuplicateSSHPublicKeyException(DuplicateSSHPublicKey,400), InvalidPublicKeyException(InvalidPublicKey,400), LimitExceededException(409), NoSuchEntityException(404), UnrecognizedPublicKeyEncodingException(UnrecognizedPublicKeyEncoding,400). SSHPublicKey.SSHPublicKeyId -> publicKeyIdType @length 20-128 @pattern ^[\w]+$ "The unique identifier for the SSH public key"; SSHPublicKey.Fingerprint -> publicKeyFingerprintType @pattern ^[:\w]+$ "The MD5 message digest of the SSH public key". DeleteSSHPublicKey(UserName, SSHPublicKeyId) -> Unit, errors NoSuchEntityException only.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay iam-ssh-public-key-lifecycle step task#9 — CreateUser, UploadSSHPublicKey, DeleteSSHPublicKey, then ListSSHPublicKeys for that user: the deleted key is still a member and never drains over 30 polls.
4. Cause 2/6: replay iam-cred-get-ssh-public-key-ssh-and-pem-encodings task#2 and iam-ssh-public-key-lifecycle task#2 — two uploads for two different users both answer SSHPublicKeyId 'APKABGXEQES5NRCCJJWAN', and the Fingerprint for the recorded key material differs from AWS's MD5 digest.
5. Cause 3: replay iam-cred-get-ssh-public-key-ssh-and-pem-encodings task#5 — upload the same SSHPublicKeyBody twice for one user and read $.ErrorResponse.Error.Message on the 400 DuplicateSSHPublicKey.
6. Cause 4: replay iam-credential-conflict-and-malformed-errors task#3 and compare SSHPublicKey.Fingerprint with AWS's digest for that key.
7. Cause 5: replay iam-ssh-public-key-error-paths task#3 — upload key material in an encoding IAM does not recognize and read $.ErrorResponse.Error.Code / .Message on the 400.
