Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-wireless-device-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-wireless-device-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetWirelessDevice|2xx->2xx|$.LoRaWAN.FPorts
Issue-Key: iotwireless|GetWirelessDevice|2xx->2xx|$.Positioning
Issue-Key: iotwireless|ListWirelessDevices|2xx->2xx|$.WirelessDeviceList[*]
Issue-Key: iotwireless|GetWirelessDevice|2xx->2xx|$.Name
Issue-Key: iotwireless|CreateWirelessDevice|4xx->4xx|$.Message
Issue-Key: iotwireless|CreateWirelessDevice|4xx->4xx|$.ResourceId
Issue-Key: iotwireless|CreateWirelessDevice|4xx->4xx|$.ResourceType
Issue-Key: iotwireless|GetWirelessDevice|4xx->4xx|$.Message
Issue-Commit: ac0b6fedb2c1d90bf91ede974c0347712fb6fd25

# GetWirelessDevice and ListWirelessDevices drop device fields, renames never apply, and both refusal messages are the emulator's own prose

> _Note: this issue was identified against the code base at commit `ac0b6fedb2c1d90bf91ede974c0347712fb6fd25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWirelessDevice

## Description
FIVE net-new root causes in the iotwireless wireless-device resource, all on one fixture. Shared fault: the wireless-device read paths report only the subset of members the create request happened to carry, and the resource's refusals are written in the emulator's own words rather than AWS's. Related but NOT covering: open issue 20261103T121946Z-iotwireless-createwirelessdevice-refusals-diverge-non-aws-deveui-conflict-message.md files the duplicate-DevEui conflict from a BEHAVIORAL check and asserts a different sentence than this wire recording does (see Cause 4) — this replay reports the step as unowned, and the two expectations must be reconciled by whoever fixes it.

Cause 1 of 5: GetWirelessDevice omits LoRaWAN.FPorts and Positioning. [fixture iot-wireless-wireless-device-lifecycle step task#6]
AWS: GetWirelessDevice (Smithy: GET /wireless-devices/{Identifier}?identifierType=..., 200; output members include LoRaWAN -> LoRaWANDevice and Positioning -> PositioningConfigStatus, values Enabled|Disabled) reports the device's FPort assignments (LoRaWANDevice.FPorts -> FPorts{Fuota,Multicast,ClockSync,Positioning{ClockSync,Gnss,Stream},Applications[]}) and a Positioning status for every LoRaWAN device. The recording shows FPorts fully populated (Positioning{ClockSync,Gnss,Stream} and an Applications entry of Type 'SemtechGeolocation') and Positioning 'Disabled'. Hypothesis: 'Disabled' is the status AWS reports for a device created without an explicit Positioning, i.e. the member is always present on a LoRaWAN device rather than echoed only when requested.
Emulator: both come back absent. The handler echoes the stored record's lorawan/positioning verbatim, so any member the create request did not carry (or that the create path did not persist) is simply missing from every later read.
Why: a client reading back FPorts to learn which port carries GNSS/stream/clock-sync frames gets null and cannot drive positioning at all; a client switching on Positioning gets an absent member where AWS guarantees one of two enum values, which is a deserialization-level surprise, not a cosmetic one.
Fix: the wireless-device state and GetWirelessDevice's output projection — persist the FPorts block on create and report Positioning with its AWS default when the request named none.

Cause 2 of 5: ListWirelessDevices' summary entries omit the members AWS reports on every device. [fixture iot-wireless-wireless-device-lifecycle step task#7]
AWS: each WirelessDeviceList entry is a WirelessDeviceStatistics (Smithy: Arn, Id, Type, Name, DestinationName, LastUplinkReceivedAt, LoRaWAN -> LoRaWANListDevice, Sidewalk, FuotaDeviceStatus, MulticastDeviceStatus, McGroupId, Positioning). The recorded entry carries Positioning 'Disabled' alongside the identity members; its FuotaDeviceStatus/MulticastDeviceStatus/McGroupId/LastUplinkReceivedAt/Sidewalk/Cellular/WiFi are shape placeholders with no value for this device and are not themselves the requirement.
Emulator: the entry contains only Arn, Id, Type, Name, DestinationName and LoRaWAN.DevEui — Positioning, which AWS reports for every device, is absent from the list view exactly as it is from the Get view.
Why: a client that lists devices to find the positioning-enabled ones cannot, and must fall back to a per-device Get that is also missing the member.
Fix: ListWirelessDevices' per-device summary builder (the WirelessDeviceStatistics projection), with the same state change Cause 1 needs.

Cause 3 of 5: a renamed device still reports its old Name, because UpdateWirelessDevice is not implemented. [fixture iot-wireless-wireless-device-lifecycle step task#9]
AWS: UpdateWirelessDevice (Smithy: PATCH /wireless-devices/{Id}, success 204, input members Name/Description/DestinationName/LoRaWAN/Positioning) mutates the stored device, and the next GetWirelessDevice reports the NEW Name.
Emulator: UpdateWirelessDevice's handler is an unimplemented scaffold that answers 501 (its own unimplemented-operation issue covers the blocked update step); the consequence filed here is that the following GetWirelessDevice returns the pre-update Name, so the read path reports stale state rather than failing.
Why: a client that renames a device and re-reads it sees the rename silently lost — no error anywhere — so mutation flows appear to work while state never changes.
Fix: UpdateWirelessDevice's handler plus the wireless-device state it must mutate, so the Get path observes the update.

Cause 4 of 5: the duplicate-DevEui ConflictException carries the emulator's prose and leaks the owning device. [fixture iot-wireless-wireless-device-lifecycle step task#10]
AWS: a CreateWirelessDevice whose LoRaWAN.DevEui is already claimed is refused HTTP 409 ConflictException (Smithy: CreateWirelessDevice errors include ConflictException @httpError 409 with members Message, ResourceId, ResourceType). In this recording the body is Message "DevEui <devEui>  with same key has been claimed" with ResourceId and ResourceType BOTH absent: the fixed text is "DevEui", then the request's DevEui value, then "  with same key has been claimed" (the recording shows two spaces before 'with'); the only variable part is the EUI copied from the request. AWS names neither the conflicting device's id nor a resource type.
Emulator: the status and code are right, but Message is the crate's own template "DevEui <devEui> is already registered to wireless device <ownerId>" and ResourceId/ResourceType are populated with the EXISTING device's id and 'WirelessDevice'.
Why: a client matching the refusal text to tell a duplicate-DevEui conflict from the idempotency-token conflict the same operation also raises as ConflictException sees an unrecognised sentence; and the emulator discloses another device's id in a field AWS leaves empty, so a client that reads ResourceId gets a value it would never get from AWS. Note the conflicting expectation recorded in the open behavioral issue above ("Device with given DevEui already exists"); both cannot be right and the fixer must pick the one its corpora agree on.
Fix: the duplicate-DevEui refusal in CreateWirelessDevice's handler — emit AWS's message text and leave ResourceId/ResourceType unset.

Cause 5 of 5: GetWirelessDevice's not-found message is the emulator's own sentence. [fixture iot-wireless-wireless-device-lifecycle step task#12]
AWS: reading a deleted/unknown wireless device is HTTP 404 ResourceNotFoundException (Smithy: @httpError 404, members Message/ResourceId/ResourceType) whose Message is the WHOLLY FIXED sentence "WirelessDevice record not found" — it names neither the identifier nor the identifier type.
Emulator: status 404 and the code are right, but Message is "No wireless device with <identifierType> <identifier>", a template filled from the request.
Why: a client branching on the refusal text to distinguish "device gone" from a validation refusal cannot match it, and the emulator's wording changes with the identifier type used, so no stable string exists to match at all.
Fix: GetWirelessDevice's not-found path — emit AWS's fixed ResourceNotFoundException message.

Traces: iot-wireless-wireless-device-lifecycle#task#6 (cause 1), #task#7 (cause 2), #task#9 (cause 3), #task#10 (cause 4), #task#12 (cause 5). Sites: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/get_wireless_device.rs:195-207 (verbatim echo of the stored record) and :180-193 (non-AWS not-found message); src/ops/list_wireless_devices.rs:207-218 (the `statistics` projection); src/ops/update_wireless_device.rs:165 (not_implemented); src/ops/create_wireless_device.rs:257-270 (the conflict message and the populated ResourceId/ResourceType) and :288-311 (the stored record, which keeps FPorts only as part of the echoed input and defaults Positioning to the request's value).

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 88 steps, 31 passed). Fixture iot-wireless-wireless-device-lifecycle, five diverging steps:

1) step task#6 GetWirelessDevice — 200 vs 200, body diverges:
   $.LoRaWAN.FPorts: expected {'Applications': [{'DestinationName': None, 'FPort': 199, 'Type': 'SemtechGeolocation'}], 'ClockSync': None, 'Fuota': None, 'Multicast': None, 'Positioning': {'ClockSync': 199, 'Gnss': 198, 'Stream': 199}}, got None
   $.Positioning: expected 'Disabled', got None

2) step task#7 ListWirelessDevices — 200 vs 200:
   $.WirelessDeviceList[0]: expected an entry carrying Positioning 'Disabled' (plus the placeholder members Cellular/FuotaDeviceStatus/LastUplinkReceivedAt/McGroupId/MulticastDeviceStatus/Sidewalk/WiFi with no value), got {'Arn': ..., 'Id': ..., 'Type': 'LoRaWAN', 'Name': ..., 'DestinationName': ..., 'LoRaWAN': {'DevEui': ...}} — Positioning absent.

3) step task#9 GetWirelessDevice after UpdateWirelessDevice(Name=<old>-renamed) — 200 vs 200:
   $.Name: expected the renamed value ('<name>-renamed'), got the original name
   $.LoRaWAN.FPorts / $.Positioning: same omissions as (1)

4) step task#10 CreateWirelessDevice reusing a claimed DevEui — 409 vs 409:
   $.Message: expected 'DevEui <devEui>  with same key has been claimed', got 'DevEui <devEui> is already registered to wireless device <existingDeviceId>'
   $.ResourceId: expected absent, got the existing device's id
   $.ResourceType: expected absent, got 'WirelessDevice'

5) step task#12 GetWirelessDevice after DeleteWirelessDevice — 404 vs 404:
   $.Message: expected 'WirelessDevice record not found', got 'No wireless device with WirelessDeviceId <deviceId>'

(The ids, names and EUIs above are per-run values from the recording, not requirements; the requirements are the status, the error code, the message templates and the presence of the members.)

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): GetWirelessDevice GET /wireless-devices/{Identifier} 200 with output {Type,Name,Description,DestinationName,Id,Arn,ThingName,ThingArn,LoRaWAN,Sidewalk,Positioning}; LoRaWANDevice{DevEui,DeviceProfileId,ServiceProfileId,OtaaV1_1,OtaaV1_0_x,AbpV1_1,AbpV1_0_x,FPorts}; FPorts{Fuota,Multicast,ClockSync,Positioning,Applications}; PositioningConfigStatus enum [Enabled,Disabled]; ListWirelessDevices GET /wireless-devices 200 with WirelessDeviceStatistics{Arn,Id,Type,Name,DestinationName,LastUplinkReceivedAt,LoRaWAN,Sidewalk,FuotaDeviceStatus,MulticastDeviceStatus,McGroupId,Positioning}; UpdateWirelessDevice PATCH /wireless-devices/{Id} 204; ConflictException{Message,ResourceId,ResourceType} 409; ResourceNotFoundException{Message,ResourceId,ResourceType} 404.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay wire fixture iot-wireless-wireless-device-lifecycle of trace set simparity-wire-aws-v1 (service iotwireless).
2. Causes 1 and 2 — CreateDestination, CreateDeviceProfile, CreateServiceProfile, then CreateWirelessDevice(Type='LoRaWAN', DestinationName=<dest>, LoRaWAN={DevEui, DeviceProfileId, ServiceProfileId, FPorts={Positioning:{ClockSync:199,Gnss:198,Stream:199}, Applications:[{FPort:199, Type:'SemtechGeolocation'}]}}). Then GetWirelessDevice(Identifier=<id>, IdentifierType='WirelessDeviceId') (step task#6): LoRaWAN.FPorts and Positioning are absent. Then ListWirelessDevices (step task#7): the entry has no Positioning member.
3. Cause 3 — UpdateWirelessDevice(Id=<id>, Name='<name>-renamed') (currently 501, its own stub issue), then GetWirelessDevice again (step task#9): Name is still the original.
4. Cause 4 — CreateWirelessDevice again with the SAME LoRaWAN.DevEui and a different Name (step task#10): the 409 body's Message is the emulator's 'DevEui X is already registered to wireless device <id>' instead of 'DevEui X  with same key has been claimed', and ResourceId/ResourceType are populated where AWS leaves them empty.
5. Cause 5 — DeleteWirelessDevice(Id=<id>), then GetWirelessDevice(Identifier=<id>, IdentifierType='WirelessDeviceId') (step task#12): the 404 Message is 'No wireless device with WirelessDeviceId <id>' instead of the fixed 'WirelessDevice record not found'.
