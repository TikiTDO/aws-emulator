Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.invalid-mfa-authentication-code-rejected iam.mfa-device-resync-code-window-enforced iam.concurrent-resync-mfa-device-race-rejected iam.resync-mfa-device-invalid-code-rejected iam.delegation-request-workflow-lifecycle iam.enable-mfa-device-wrong-code-rejected iam.entity-temporarily-unmodifiable-enforced iam.delete-with-dependents-returns-deleteconflict
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.invalid-mfa-authentication-code-rejected|run#17|AssertionError|assert code_bad == "InvalidAuthenticationCode", (
Issue-Key: iam.mfa-device-resync-code-window-enforced|run#13|AssertionError|assert code == "InvalidAuthenticationCode", f"expected InvalidAuthenticationCode, got {code}"
Issue-Key: iam.concurrent-resync-mfa-device-race-rejected|run#23|AssertionError|assert code in acceptable_failure_codes, (
Issue-Key: iam.resync-mfa-device-invalid-code-rejected|_resync_with_retry#16|Timeout|time.sleep(wait)
Issue-Key: iam.delegation-request-workflow-lifecycle|run#29|AssertionError|assert code_send == "NoSuchEntity", (
Issue-Key: iam.enable-mfa-device-wrong-code-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iam.entity-temporarily-unmodifiable-enforced|_assert_one_of#17|AssertionError|assert message == expected[code], (
Issue-Key: iam.delete-with-dependents-returns-deleteconflict|run#111|AssertionError|assert code_mfa == "DeleteConflict", f"DeleteVirtualMFADevice: expected DeleteConflict, got {code_mfa}"
Issue-Commit: 0275b87193e8729c4e6e84626c64cbadccfa7c1a

# iam: 5 net-new causes — ResyncMFADevice and SendDelegationToken answer NotImplemented, EnableMFADevice's InvalidAuthenticationCode and NoSuchEntity messages are emulator prose, and DeleteVirtualMFADevice never refuses DeleteConflict

> _Note: this issue was identified against the code base at commit `0275b87193e8729c4e6e84626c64cbadccfa7c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResyncMFADevice

## Description
Five distinct root causes on the IAM MFA and delegation surface. Related open issues that do NOT carry these checks' replay keys: 20261105T054223Z-iam-3-net-new-root-causes-createvirtualmfadevice-s.md, 20261027T214234Z-iam-6-net-new-behavioral-root-causes-enablemfadevice.md, and 20261105T054223Z-iam-deletevirtualmfadevice-stub.md. A fixer must close all five before submitting.

=== Cause 1 of 5: ResyncMFADevice is a scaffolded stub ===
AWS: Smithy — ResyncMFADevice(UserName, SerialNumber, AuthenticationCode1, AuthenticationCode2) returns an empty 200 body on success. Its modelled errors are ConcurrentModificationException (awsQueryError ConcurrentModification, 409), InvalidAuthenticationCodeException (InvalidAuthenticationCode, 403), LimitExceededException (LimitExceeded, 409), NoSuchEntityException (NoSuchEntity, 404) and ServiceFailureException (ServiceFailure, 500). A wrong or out-of-window pair of authentication codes is refused 403 InvalidAuthenticationCode; two concurrent resyncs of the same device resolve as one success and one refusal that is either 409 ConcurrentModification or 403 InvalidAuthenticationCode; the device must stay associated with the user across a rejected resync.
Emulator: every ResyncMFADevice call answers error code NotImplemented ("operation ResyncMFADevice is scaffolded but not implemented in this emulator"), which is outside the modelled error set for this operation.
Why: a client cannot re-synchronise a drifted MFA device at all, and the error code it gets back is not one its SDK's error factory models, so retry and branch logic keyed on InvalidAuthenticationCode or ConcurrentModification never fires.
Fix: the iam ResyncMFADevice handler (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/resync_mfa_device.rs) and the MFA-device record in service state.
Traces: iam.invalid-mfa-authentication-code-rejected, iam.mfa-device-resync-code-window-enforced, iam.concurrent-resync-mfa-device-race-rejected, iam.resync-mfa-device-invalid-code-rejected

=== Cause 2 of 5: SendDelegationToken is a scaffolded stub ===
AWS: Smithy — SendDelegationToken(DelegationRequestId) returns an empty 200 body; its modelled errors are ConcurrentModificationException (409), InvalidInputException (InvalidInput, 400), NoSuchEntityException (NoSuchEntity, 404) and ServiceFailureException (500). A DelegationRequestId that names no delegation request is refused 404 NoSuchEntity.
Emulator: SendDelegationToken answers NotImplemented, so the check's expected NoSuchEntity refusal never occurs.
Why: the delegation-request workflow cannot be driven at all, and a client that distinguishes "no such delegation request" from a transport failure cannot, because NotImplemented is not a modelled outcome of this operation.
Fix: the iam SendDelegationToken handler (services/.../src/ops/send_delegation_token.rs) and the delegation-request records in service state.
Traces: iam.delegation-request-workflow-lifecycle

=== Cause 3 of 5: EnableMFADevice's InvalidAuthenticationCode message is emulator prose ===
AWS: Smithy — EnableMFADevice lists InvalidAuthenticationCodeException (awsQueryError InvalidAuthenticationCode, HTTP 403). Recorded behaviour: a wrong authentication-code pair is refused 403 InvalidAuthenticationCode with the message "Authentication code for device is not valid." — the whole string is fixed text with no variable part.
Emulator: the code is correct but the message reads "The authentication code was not recognized or has expired. Please try again with a new code."
Why: the message is a requirement of the error class; a client or operator matching AWS's wording (log scrapers, user-facing surfacing of the IAM message) does not match, and the emulator's text additionally asserts an expiry that AWS does not state.
Fix: the iam EnableMFADevice handler's authentication-code validation message (services/.../src/ops/enable_mfa_device.rs).
Traces: iam.enable-mfa-device-wrong-code-rejected

=== Cause 4 of 5: EnableMFADevice's NoSuchEntity message for an unknown serial number is emulator prose ===
AWS: Smithy — EnableMFADevice lists NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404). Recorded behaviour: a SerialNumber that names no MFA device is refused 404 NoSuchEntity with the message "MFA Device with serial number <SerialNumber> does not exist." — fixed text "MFA Device with serial number " ... " does not exist." with the serial number taken verbatim from the request; the user name does not appear.
Emulator: the message reads "MFA Device <SerialNumber> cannot be found for user <UserName>." — different fixed text, and it names the request's UserName, which AWS's message for this class does not.
Why: the message is a requirement of the error class; besides breaking any client matching AWS's wording, the emulator's variant leaks the user name into a not-found message where AWS does not.
Fix: the iam EnableMFADevice handler's device-lookup refusal message (services/.../src/ops/enable_mfa_device.rs).
Traces: iam.entity-temporarily-unmodifiable-enforced

=== Cause 5 of 5: DeleteVirtualMFADevice never refuses DeleteConflict for a device still enabled on a user ===
AWS: Smithy — DeleteVirtualMFADevice(SerialNumber) lists DeleteConflictException (awsQueryError DeleteConflict, HTTP 409). Recorded behaviour: deleting a virtual MFA device that is still enabled on a user is refused 409 DeleteConflict; the device must first be deactivated.
Emulator: DeleteVirtualMFADevice answers NotImplemented, so the dependent-resource refusal never happens and a client cannot distinguish "still in use" from any other failure.
Why: the delete-with-dependents contract — every IAM delete of an entity that still has dependents answers DeleteConflict — is broken for this entity type, so a client's cleanup ordering logic gets no signal.
Fix: the iam DeleteVirtualMFADevice handler (services/.../src/ops/delete_virtual_mfa_device.rs) must both delete and enforce the enabled-device DeleteConflict refusal. The open issue 20261105T054223Z-iam-deletevirtualmfadevice-stub.md covers implementing the operation; this cause additionally fixes the DeleteConflict refusal that this check asserts, and carries that check's replay key.
Traces: iam.delete-with-dependents-returns-deleteconflict

## Evidence
Cause 1 —
  iam.invalid-mfa-authentication-code-rejected: AssertionError: expected InvalidAuthenticationCode for a wrong resync code, got NotImplemented
  iam.mfa-device-resync-code-window-enforced: AssertionError: expected InvalidAuthenticationCode, got NotImplemented
  iam.concurrent-resync-mfa-device-race-rejected: AssertionError: unexpected losing-racer error code 'NotImplemented' (expected ConcurrentModificationException or InvalidAuthenticationCodeException)
  iam.resync-mfa-device-invalid-code-rejected: Error: Check timed out after 60.0s
  Expected: 403 InvalidAuthenticationCode (or 409 ConcurrentModification for the losing racer); the device stays associated with the user.

Cause 2 —
  AssertionError: expected NoSuchEntity for SendDelegationToken, got NotImplemented
  Expected: 404 NoSuchEntity.

Cause 3 —
  AssertionError: expected message 'Authentication code for device is not valid.', got 'The authentication code was not recognized or has expired. Please try again with a new code.'
  Status/code observed correct (403 InvalidAuthenticationCode); only the message diverges.

Cause 4 —
  AssertionError: EnableMFADevice: expected message 'MFA Device with serial number <serial> does not exist.' for NoSuchEntity, got 'MFA Device <serial> cannot be found for user <user>.'
  (the serial and user values shown in the replay are per-run names; the fixed text and the placement of the serial number are the requirement)

Cause 5 —
  AssertionError: DeleteVirtualMFADevice: expected DeleteConflict, got NotImplemented
  Expected: 409 DeleteConflict while the device is still enabled on the user.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: CreateUser, CreateVirtualMFADevice, EnableMFADevice, then call ResyncMFADevice with a wrong AuthenticationCode1/2 pair — and again with two concurrent resyncs of the same device.
3. Cause 2: call SendDelegationToken with a DelegationRequestId that names no delegation request.
4. Cause 3: call EnableMFADevice with a valid user and serial number but a wrong authentication-code pair; read Error.Message.
5. Cause 4: call EnableMFADevice with a SerialNumber that names no MFA device; read Error.Message.
6. Cause 5: CreateVirtualMFADevice, EnableMFADevice on a user, then DeleteVirtualMFADevice without deactivating it.
