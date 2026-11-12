Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.import-host-key-duplicate-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.import-host-key-duplicate-rejected|run#12|AssertionError|assert message == "Host key already exists", f"unexpected duplicate-host-key message: {message!r}"
Issue-Commit: 8d9c0a623e7fe7811db752ce36b78a02deee9e39

# Transfer ImportHostKey duplicate-key refusal returns emulator-authored prose instead of AWS's fixed "Host key already exists" message

> _Note: this issue was identified against the code base at commit `8d9c0a623e7fe7811db752ce36b78a02deee9e39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHostKey

## Description
One net-new root cause in the transfer service, grounded in a failing behavioral check of this suite.

=== Cause 1 of 1: the ResourceExistsException message for a duplicate ImportHostKey is emulator-invented text built from request and state values ===
AWS: Smithy (com.amazonaws.transfer#TransferService) ImportHostKey takes ServerId (@required, @length 19, pattern ^s-([0-9a-f]{17})$) and HostKeyBody (@required, target HostKey, @length 0..4096, @sensitive) and models ResourceExistsException as a client error with httpError 409 (alongside InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429, InternalServiceError/ServiceUnavailableException 503). The model carries no message template, so for the text this is recorded behavior only: re-importing the exact same key material onto the same server is refused with code ResourceExistsException and Message exactly "Host key already exists". That message is ENTIRELY fixed service text — it has no variable parts at all: no server id, no host key id, no fingerprint, no limit.
Emulator: the HTTP status and the error code are right (ResourceExistsException), but the Message is a different, emulator-authored sentence that interpolates request and state values: "Server <ServerId> already holds host key <HostKeyId> with fingerprint SHA256:<base64-digest>". In that string "Server ", " already holds host key " and " with fingerprint SHA256:" are the emulator's own fixed text, while the server id comes from the request, the host key id comes from the emulator's stored host-key record for the colliding key, and the digest is computed from the submitted HostKeyBody. AWS supplies none of those values on this path. (One observed instance, whose ids/digest are per-run values and not part of the contract: "Server s-00000000000000001 already holds host key hostkey-00000000000000003 with fingerprint SHA256:dMdRTFV5f6ZsGXd8q7q2Ezi5WRo4vVA3VcLcUNFGznY".)
Why: the error message is part of the error contract. A client that recognises the duplicate-host-key condition by matching the message (string equality, a regex, or a log/alarm assertion) does not recognise it against the emulator and treats a well-understood 409 as an unknown failure. The emulator's message additionally discloses the colliding host key id and a fingerprint of the submitted private key, state AWS does not return on this path.
Fix: the ResourceExistsException message construction in the ImportHostKey handler's duplicate-detection/error path of the transfer service crate (services/service-com-amazonaws-transfer-transferservice/src/ops/import_host_key.rs and whatever error helper it formats through) — the duplicate refusal must emit the fixed text and nothing interpolated. Note: open issue 20261026T022407Z-transfer-host-key-and-web-app-planes-still.md describes this same message rule under its Cause 2, but it is keyed to the check transfer.import-hostkey-duplicate-resource-exists-rejected and does not carry this check's replay key; the suite reports transfer.import-host-key-duplicate-rejected as unowned, so it is filed here. Fixing the message once should close both.
Traces: transfer.import-host-key-duplicate-rejected.

## Evidence
Check transfer.import-host-key-duplicate-rejected (mismatch).

Sequence: create_server(Tags=[...]) -> ServerId; import_host_key(ServerId, HostKeyBody=<RSA-2048 PEM PKCS#1/TraditionalOpenSSL key>, Description='k1') succeeded and returned a HostKeyId; the second import_host_key(ServerId, HostKeyBody=<the identical key>, Description='k1-dup') was then refused.

Expected: Error.Code == 'ResourceExistsException' (HTTP 409) with Error.Message exactly 'Host key already exists' (wholly fixed text, no interpolated values).
Actual: Error.Code == 'ResourceExistsException' (correct) with Error.Message = 'Server s-00000000000000001 already holds host key hostkey-00000000000000003 with fingerprint SHA256:dMdRTFV5f6ZsGXd8q7q2Ezi5WRo4vVA3VcLcUNFGznY'.

Failing assertion, check_transfer_import_host_key_duplicate_rejected.py line 53 (source line 34 of run):
  assert message == "Host key already exists", f"unexpected duplicate-host-key message: {message!r}"
AssertionError: unexpected duplicate-host-key message: 'Server s-00000000000000001 already holds host key hostkey-00000000000000003 with fingerprint SHA256:dMdRTFV5f6ZsGXd8q7q2Ezi5WRo4vVA3VcLcUNFGznY'

(The ids and the digest above are per-run values; the requirement is the fixed text, not these values.)

## Steps to Reproduce
1. Boot the emulator and point botocore's transfer client at it.
2. server_id = transfer.create_server(Tags=[{'Key':'sim2real','Value':'import-host-key-duplicate-rejected'}])['ServerId']
3. Generate an RSA-2048 private key and serialize it with cryptography Encoding.PEM + PrivateFormat.TraditionalOpenSSL + NoEncryption; call transfer.import_host_key(ServerId=server_id, HostKeyBody=pem, Description='k1') and note the returned HostKeyId (this call succeeds).
4. Call transfer.import_host_key(ServerId=server_id, HostKeyBody=pem, Description='k1-dup') with the IDENTICAL HostKeyBody and catch the ClientError.
5. Read exc.response['Error']['Code'] and ['Message']: the Code is ResourceExistsException (correct) but the Message is 'Server <server id> already holds host key <host key id> with fingerprint SHA256:<digest>' instead of the fixed 'Host key already exists'.
6. Confirm the effect is otherwise right: list_host_keys(ServerId=server_id) shows only the originally imported key, and importing genuinely different key material still succeeds.
