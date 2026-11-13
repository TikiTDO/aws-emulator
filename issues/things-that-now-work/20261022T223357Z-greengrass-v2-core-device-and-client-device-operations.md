Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-core-device-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-core-device-error-paths greengrassv2-list-variations
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|GetCoreDevice|4xx->5xx|$.message
Issue-Key: greengrass|GetCoreDevice|4xx->5xx|$headers.content-type
Issue-Key: greengrass|GetCoreDevice|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|GetCoreDevice|4xx->5xx|status
Issue-Key: greengrass|DeleteCoreDevice|4xx->5xx|$.message
Issue-Key: greengrass|DeleteCoreDevice|4xx->5xx|$headers.content-type
Issue-Key: greengrass|DeleteCoreDevice|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|DeleteCoreDevice|4xx->5xx|status
Issue-Key: greengrass|ListClientDevicesAssociatedWithCoreDevice|2xx->5xx|$.associatedClientDevices
Issue-Key: greengrass|ListClientDevicesAssociatedWithCoreDevice|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListClientDevicesAssociatedWithCoreDevice|2xx->5xx|status
Issue-Key: greengrass|ListInstalledComponents|2xx->5xx|$
Issue-Key: greengrass|ListInstalledComponents|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListInstalledComponents|2xx->5xx|status
Issue-Key: greengrass|ListEffectiveDeployments|2xx->5xx|$
Issue-Key: greengrass|ListEffectiveDeployments|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListEffectiveDeployments|2xx->5xx|status
Issue-Key: greengrass|BatchAssociateClientDeviceWithCoreDevice|2xx->5xx|$.errorEntries
Issue-Key: greengrass|BatchAssociateClientDeviceWithCoreDevice|2xx->5xx|$headers.content-type
Issue-Key: greengrass|BatchAssociateClientDeviceWithCoreDevice|2xx->5xx|status
Issue-Key: greengrass|BatchDisassociateClientDeviceFromCoreDevice|2xx->5xx|$.errorEntries
Issue-Key: greengrass|BatchDisassociateClientDeviceFromCoreDevice|2xx->5xx|$headers.content-type
Issue-Key: greengrass|BatchDisassociateClientDeviceFromCoreDevice|2xx->5xx|status
Issue-Key: greengrass|ListCoreDevices|2xx->5xx|$.coreDevices
Issue-Key: greengrass|ListCoreDevices|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListCoreDevices|2xx->5xx|status
Issue-Commit: 6e0b8b1107b57545eaa8864803a2ad1310139146

# Greengrass V2 core device and client device operations return 500 AmbiguousService before dispatch

> _Note: this issue was identified against the code base at commit `6e0b8b1107b57545eaa8864803a2ad1310139146`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCoreDevice

## Description
This bundle carries 8 causes, all on the Greengrass V2 core-device / client-device surface. Shared fault: the emulator's router keys emulated services by endpoint prefix, and Greengrass (2017-06-07) and GreengrassV2 (2020-11-30) both declare endpointPrefix/arnNamespace/sigv4Name 'greengrass' (Smithy: service_info on both models), so every request is refused before any handler runs with HTTP 500, content-type application/x-amz-json-1.1 and body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'greengrass': ... rejected rather than served by an arbitrary one"}. AWS co-hosts both versions on greengrass.<region>.amazonaws.com and routes by the restJson1 @http method+uri binding. The open behavioral-corpus issue 20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass.md names the same root cause but carries no wire replay key, so these fixture bindings are net-new.

Cause 1 of 8 - GetCoreDevice on a nonexistent core device.
AWS: Smithy GreengrassV2.GetCoreDevice, GET /greengrass/v2/coreDevices/{coreDeviceThingName}, success 200, modeled errors include ResourceNotFoundException httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, content-type application/json, message (example) '<thing-name> can not be found'.
Emulator: 500, x-amzn-errortype AmbiguousService.
Why: a client cannot tell an unregistered core device from a server fault, so existence probes retry a permanent condition.
Fix: the request router / service-registry dispatch layer (no handler is reached), plus the restJson1 error serializer emitting x-amz-json-1.1.
Traces: greengrassv2-core-device-error-paths#task#1.

Cause 2 of 8 - DeleteCoreDevice on a nonexistent core device.
AWS: ResourceNotFoundException modeled, httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) '<thing-name> can not be found'.
Emulator: 500 / AmbiguousService.
Why: deregistration cleanup cannot treat an absent device as already gone.
Fix: same dispatch layer; then DeleteCoreDevice's lookup.
Traces: greengrassv2-core-device-error-paths#task#2.

Cause 3 of 8 - ListClientDevicesAssociatedWithCoreDevice.
AWS: the operation answers 200 application/json with an associatedClientDevices list; recorded response for a core device with no associations is 200 with associatedClientDevices = [].
Emulator: 500 / AmbiguousService, associatedClientDevices absent.
Why: a client cannot read the association set, and an empty result is indistinguishable from a failure.
Fix: same dispatch layer; then the operation's handler/state.
Traces: greengrassv2-core-device-error-paths#task#3 (also #task#7, #task#9).

Cause 4 of 8 - ListInstalledComponents.
AWS: answers 200 application/json; recorded response is 200 with an empty JSON object body for a device reporting no installed components.
Emulator: 500 with the AmbiguousService error object as the body.
Why: fleet inventory reads fail outright, so nothing can confirm what a core device is running.
Fix: same dispatch layer; then the operation's handler/serializer.
Traces: greengrassv2-core-device-error-paths#task#4.

Cause 5 of 8 - ListEffectiveDeployments.
AWS: answers 200 application/json; recorded response is 200 with an empty JSON object body when a core device has no effective deployments.
Emulator: 500 with the AmbiguousService error object as the body.
Why: a client cannot determine which deployment is effective on a device, which is the only read-back for deployment convergence.
Fix: same dispatch layer; then the operation's handler/serializer.
Traces: greengrassv2-core-device-error-paths#task#5.

Cause 6 of 8 - BatchAssociateClientDeviceWithCoreDevice.
AWS: answers 200 application/json with an errorEntries list; recorded response is 200 with errorEntries = [] (per-entry failures are reported inside errorEntries, not as a request-level error).
Emulator: 500 / AmbiguousService, errorEntries absent.
Why: a client cannot associate client devices, and cannot read the per-entry error channel the API uses instead of failing the call.
Fix: same dispatch layer; then the operation's handler/serializer.
Traces: greengrassv2-core-device-error-paths#task#6.

Cause 7 of 8 - BatchDisassociateClientDeviceFromCoreDevice.
AWS: answers 200 application/json with an errorEntries list; recorded response is 200 with errorEntries = [].
Emulator: 500 / AmbiguousService, errorEntries absent.
Why: disassociation cannot be performed or verified, so association state can never be unwound.
Fix: same dispatch layer; then the operation's handler/serializer.
Traces: greengrassv2-core-device-error-paths#task#8.

Cause 8 of 8 - ListCoreDevices.
AWS: Smithy GreengrassV2.ListCoreDevices, GET /greengrass/v2/coreDevices, success 200, output coreDevices + nextToken, paginated (items=coreDevices, pageSize=maxResults, range 1..100), with thingGroupArn/status/runtime httpQuery filters. Recorded response: 200 with coreDevices = [].
Emulator: 500 / AmbiguousService, coreDevices absent.
Why: fleet enumeration, its filters and its pagination are all unreachable.
Fix: same dispatch layer; then ListCoreDevices' handler/serializer.
Traces: greengrassv2-core-device-error-paths#task#10 (also #task#11, greengrassv2-list-variations#task#6).

## Evidence
Expected (AWS / recorded): GetCoreDevice and DeleteCoreDevice on a missing device -> 404 ResourceNotFoundException, content-type application/json; ListClientDevicesAssociatedWithCoreDevice -> 200 with associatedClientDevices []; ListInstalledComponents and ListEffectiveDeployments -> 200 with an empty JSON object; BatchAssociate/BatchDisassociateClientDevice -> 200 with errorEntries []; ListCoreDevices -> 200 with coreDevices [].

Actual (emulator): all eight answer HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService, body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one"}. For the 200-expected steps the expected members (associatedClientDevices, errorEntries, coreDevices) are entirely absent; for ListInstalledComponents / ListEffectiveDeployments the whole body is the error object where {} was recorded. Replay: 47 mismatched steps, score 6.8%, state_reset_failures 0.

Model citation: smithy_query operation ListCoreDevices/GetCoreDevice on services/service-com-amazonaws-greengrassv2-greengrassv2 (restJson1, success 200, ResourceNotFoundException httpError 404); service_info shows endpointPrefix 'greengrass' on both the greengrass and greengrassv2 models, which is what the router refuses to disambiguate. AmbiguousService is not a modeled error of either service.

## Steps to Reproduce
1. Boot the emulator from this checkout with its default service set (greengrass and greengrassv2 both claim the 'greengrass' endpoint prefix).
2. Replay fixture greengrassv2-core-device-error-paths, step task#1 (GetCoreDevice on a nonexistent device): expected 404 ResourceNotFoundException, observed 500 AmbiguousService.
3. Replay the same fixture's task#2 (DeleteCoreDevice 404), task#3 (ListClientDevicesAssociatedWithCoreDevice 200), task#4 (ListInstalledComponents 200), task#5 (ListEffectiveDeployments 200), task#6 (BatchAssociateClientDeviceWithCoreDevice 200), task#8 (BatchDisassociateClientDeviceFromCoreDevice 200) and task#10 (ListCoreDevices 200).
4. Observe every step returns 500 with content-type application/x-amz-json-1.1 and errortype AmbiguousService instead of the modeled status and shape.
5. All 8 causes must answer with their modeled status/code/shape before this issue is closed.
