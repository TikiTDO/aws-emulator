Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.duplicate-dev-eui-rejected-within-account
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.duplicate-dev-eui-rejected-within-account|run#22|AssertionError|assert message == "Device with given DevEui already exists", f"unexpected message {message!r}"
Issue-Commit: df5769e92d3730d0dcd5c18e06d2c5be1d7ce2eb

# CreateWirelessDevice duplicate-DevEui ConflictException carries non-AWS message text

> _Note: this issue was identified against the code base at commit `df5769e92d3730d0dcd5c18e06d2c5be1d7ce2eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessDevice

## Description
One net-new cause.

Cause 1 of 1: a second CreateWirelessDevice that reuses a DevEui already claimed in the account is refused with the right error code and status but with the emulator's own sentence instead of AWS's.

AWS: 409 ConflictException (Smithy: CreateWirelessDevice errors include ConflictException @httpError 409; ConflictException { Message (length 0..2048), ResourceId, ResourceType } with no jsonName, so the wire key is `Message`; success status for the operation is 201). The refusal message for a DevEui already registered in the account is recorded behavior only, and the recording shows it as a FIXED sentence with NO variable parts at all: "Device with given DevEui already exists". The rejected DevEui is not echoed, and neither is any identity of the device that holds it.

Emulator: the status (409) and code (ConflictException) match, but the Message is "DevEui <devEui>  with same key has been claimed" — a different sentence that additionally interpolates the DevEui from the request and contains a doubled space between the identifier and "with". In the replayed check this surfaced as AssertionError: unexpected message 'DevEui ec09b3e0aa8db7cb  with same key has been claimed' (that hex value is the per-run DevEui the check generated, not part of the requirement; the requirement is the fixed AWS sentence with no interpolation).

Why: a client or test that reads the refusal text to distinguish the duplicate-DevEui conflict from the other ConflictException the same operation raises (a replayed @idempotencyToken ClientRequestToken used with different parameters) matches on AWS's wording and sees no match here; error-message assertions and log/alarm patterns keyed on AWS's sentence fail against the emulator for an otherwise correct request/response pair. The interpolated DevEui also discloses the request value inside a message AWS keeps value-free.

Fix: the duplicate-DevEui refusal body shared by this crate's wireless-device create path — the helper that composes the ConflictException message for an already-claimed DevEui, which CreateWirelessDevice's handler calls before minting the device. The surrounding status, code and conflict detection are already correct; only the message text diverges. Note the already-open issue "iotwireless validation refusals carry no Message and wrong enum value set wording" (20261029T171855Z-iotwireless-validation-refusals-carry-no-message-and-wrong.md) covers ValidationException bodies on create operations and does NOT cover this ConflictException sentence.

Traces: behavioral check iotwireless.duplicate-dev-eui-rejected-within-account.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check iotwireless.duplicate-dev-eui-rejected-within-account (7/8 iotwireless checks passed; this one mismatched):

  check_iotwireless_duplicate_dev_eui_rejected_within_account.py, line 105, in run
    assert message == "Device with given DevEui already exists", f"unexpected message {message!r}"
  AssertionError: unexpected message 'DevEui ec09b3e0aa8db7cb  with same key has been claimed'

Expected (AWS, recorded behavior): HTTP 409, Error.Code = "ConflictException", Error.Message = "Device with given DevEui already exists" — entirely fixed text, no request- or state-derived parts.
Actual (emulator): HTTP 409, Error.Code = "ConflictException" (matches), Error.Message = "DevEui <devEui from request>  with same key has been claimed" (two spaces before "with").

The preceding assertion `code == "ConflictException"` passed, and the subsequent assertions passed too (the claimed DevEui still resolves to the first device, and a sibling DevEui registers as a distinct device), so detection and state are correct and only the message text diverges.

Crate evidence: services/service-com-amazonaws-iotwireless-iotwireless/src/state.rs:313-315 — `duplicate_dev_eui_message` formats "DevEui {dev_eui}  with same key has been claimed"; state.rs:323-329 — `duplicate_dev_eui` builds the ConflictException from it; services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_device.rs:274-281 — the handler returns it when `store.dev_eui_index` already holds the DevEui.
Model evidence: services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json — ConflictException @httpError 409 with members Message/ResourceId/ResourceType; CreateWirelessDevice http { POST /wireless-devices, code 201 }.

## Steps to Reproduce
1. Boot the emulator built from this checkout and run the behavioral suite simparity-behavioral-aws-v1.
2. Create a device profile (LoRaWAN MacVersion 1.0.3), a service profile, an IAM role and a destination (ExpressionType RuleName).
3. Call CreateWirelessDevice with Type=LoRaWAN, that DestinationName, and LoRaWAN { DeviceProfileId, ServiceProfileId, DevEui=<a fresh 8-byte hex EUI>, OtaaV1_0_x { AppKey, AppEui } } — it succeeds.
4. Call CreateWirelessDevice again with a different Name but the SAME DevEui (and fresh OTAA key material).
5. Read the 409 body: Error.Code is ConflictException (correct) but Error.Message is "DevEui <devEui>  with same key has been claimed" instead of AWS's fixed "Device with given DevEui already exists".
6. Equivalent: replay behavioral check iotwireless.duplicate-dev-eui-rejected-within-account.
