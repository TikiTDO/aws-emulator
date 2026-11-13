Issue-Source: trace-sweep-divergence
Issue-Trace: iotdeviceadvisor-get-endpoint-returns-device-advisor-domain
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdeviceadvisor-get-endpoint-returns-device-advisor-domain
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor|GetEndpoint|2xx->2xx|$.endpoint
Issue-Commit: 7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d

# iotdeviceadvisor GetEndpoint returns the account-level endpoint for a request that names a device

> _Note: this issue was identified against the code base at commit `7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEndpoint

## Description
Cause 1 of 1: GetEndpoint vends the ACCOUNT-LEVEL Device Advisor host for a request that names a device under test, where AWS vends that device's own host.

AWS: Smithy (services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json): GetEndpoint is GET /endpoint, success 200; its input members thingArn, certificateArn and deviceRoleArn are each @httpQuery AmazonResourceName (@length 20..2048) and authenticationMethod is an @httpQuery enum; its output GetEndpointResponse has the single member endpoint (shape Endpoint, @length 45..75), documented as "The response of an Device Advisor endpoint". Errors are InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). The model documents each device member as "an optional parameter", and the recorded response shows the distinction AWS draws between the two endpoints it vends: for the recorded request the 200 body's endpoint is a host whose first DNS label is a per-DEVICE label (a short lowercase alphanumeric label), under .deviceadvisor.iot.<region>.amazonaws.com — i.e. the DEVICE-LEVEL endpoint, not the account-scoped one whose label is the account id. Requirement: when the request names a device by any of thingArn / certificateArn / deviceRoleArn, the endpoint returned is that device's own host; the account-labelled host is returned only for a request that names no device. The concrete label value in the recording (and the account id, region, host and port of any environment) is a per-environment value, not part of the requirement.

Emulator: the replay returns 200 (status matches) but $.endpoint carries the ACCOUNT-level host - the first label is the emulator account id (verdict `mismatch`, detail `$.endpoint: host_not_own`, expected a per-device label, got `<account-id>.deviceadvisor.iot.<region>.amazonaws.com`). So for a device-naming request the emulator answers the host it is supposed to reserve for a request that names no device, and every device in an account collapses onto that one host.

Why: a client that calls GetEndpoint to learn where to point a device under test is handed the account-level host instead of the device's host, so the device connects to the wrong Device Advisor endpoint and the test session never binds to the device the caller named. Because the answer no longer varies with the device, a client that keys a per-device connection or a per-device endpoint cache on the result sees every device share one endpoint, which is indistinguishable from the account-level case and cannot be detected client-side (the call still succeeds with 200).

Fix: the GetEndpoint handler and the shared endpoint derivation it delegates to - services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/get_endpoint.rs builds a crate::state::DeviceIdentity from input.thing_arn / input.certificate_arn / input.device_role_arn and passes it to crate::state::endpoint_for, which branches on DeviceIdentity::names_no_device() between endpoint_for_region (account label) and endpoint_for_device (per-device label). The emulator took the account branch, so either the identity reaching the handler is empty or the branch is not selecting the device derivation. Hypothesis: the @httpQuery device members are not being bound off the query string into the input, leaving all three None and names_no_device() true; whichever it is, the fix belongs in that input binding / handler branch, not in the host-formatting helper (the domain shape and length bounds are already correct). StartSuiteRun reports its run's endpoint through the same derivation, so fixing it there keeps the two reports consistent.

Traces: iotdeviceadvisor-get-endpoint-returns-device-advisor-domain#task#1.

## Evidence
Fixture iotdeviceadvisor-get-endpoint-returns-device-advisor-domain, step task#1, operation GetEndpoint, service iotdeviceadvisor.
Expected: HTTP 200, body $.endpoint = a DEVICE-LEVEL host, i.e. a per-device DNS label followed by `.deviceadvisor.iot.<region>.amazonaws.com` (recording shows a 14-character lowercase-alphanumeric label; that exact label is a per-environment value).
Actual:   HTTP 200, body $.endpoint = the ACCOUNT-LEVEL host `<account-id>.deviceadvisor.iot.<region>.amazonaws.com` (recorded run: `000000000000.deviceadvisor.iot.us-west-2.amazonaws.com`).
Replay verdict: mismatch, divergence_key `service=iotdeviceadvisor|GetEndpoint|status=200->200|keys=$.endpoint`, detail `$.endpoint: host_not_own`. Status, error code and error shape are not in question here - only the value of the single modeled output member `endpoint`, whose first DNS label identifies the account rather than the device the request named.
No open issue covers this (the only iotdeviceadvisor-mentioning open issue is the cloudformation one about AWS::IoTCoreDeviceAdvisor::SuiteDefinition resource types, a different service and operation).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `iotdeviceadvisor-get-endpoint-returns-device-advisor-domain`, step `task#1`.
2. Equivalently, by hand: call GetEndpoint (GET /endpoint) against the iotdeviceadvisor endpoint with a device named in the query string, e.g. `?thingArn=arn:aws:iot:us-west-2:<account>:thing/<thing-name>` (optionally with `authenticationMethod=SignatureVersion4`), signed for us-west-2.
3. Observe the 200 response body's `endpoint`: its first DNS label is the account id, i.e. the account-level host.
4. Call GetEndpoint again with NO device member at all. Observe the same host is returned, showing the device-level derivation is never selected.
5. Expected: the device-naming call returns a per-device host (a device label under `.deviceadvisor.iot.<region>.amazonaws.com`, stable for the same device and different from the account-level host), and only the device-less call returns the account-labelled host.
