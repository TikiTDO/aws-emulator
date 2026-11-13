Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain|run#4|AssertionError|assert 45 <= len(endpoint) <= 75, f"expected endpoint length in [45, 75], got {len(endpoint)}: {endpoint!r}"
Issue-Commit: 1243a911f95c5632028c180747e4c8066bbee888

# iotdeviceadvisor GetEndpoint vends the emulator's own loopback host:port instead of a Device Advisor DNS endpoint, violating the modeled Endpoint length bound

> _Note: this issue was identified against the code base at commit `1243a911f95c5632028c180747e4c8066bbee888`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEndpoint

## Description
Cause 1 of 1: the account-level GetEndpoint call returns the raw address the request reached the emulator on (a bare `host:port`) rather than a Device Advisor regional DNS hostname, so the single modeled output member `endpoint` is both out of its modeled length range and not a Device Advisor domain. Failing check: iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain.

AWS: Smithy (services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/smithy-model/com-amazonaws-iotdeviceadvisor-iotsenateservice.json): `GetEndpoint` is `GET /endpoint`, success 200, output `GetEndpointResponse` with the single member `endpoint` targeting shape `Endpoint` with `@length min 45, max 75`, documented as "The response of an Device Advisor endpoint"; modeled errors are InternalServerException (500), ResourceNotFoundException (404), ValidationException (400). All four input members (`thingArn`, `certificateArn`, `deviceRoleArn`, `authenticationMethod`) are optional `@httpQuery` members, so a call with no query string is the legal account-level form. AWS answers it 200 with a DNS hostname of the regional Device Advisor domain — a single leading label followed by `.deviceadvisor.iot.<region>.amazonaws.com` (the Device Advisor / device-gateway domain), which is what puts the value inside the modeled 45..75 bound and what embeds the region the request was signed for. The leading label, the region and the account id are values taken from the request and the caller's account; they are not part of the requirement. The requirement is the FORM: a hostname (no scheme, no path), 45..75 characters, containing the request's region and the Device Advisor domain, i.e. resolvable by a client that resolves AWS names.

Emulator: the replay of `get_endpoint()` (no query parameters) returns 200 with `endpoint` = `127.0.0.1:32893` — 15 characters, a bare IPv4 literal plus a port, with no region substring and no `deviceadvisor`/`devicegateway` label. The check's first assertion is the one that fires: `AssertionError: expected endpoint length in [45, 75], got 15: '127.0.0.1:32893'`. The behaviour is deliberate in the current code: services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/get_endpoint.rs calls `crate::state::announced_endpoint(...)`, and services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state/endpoint_addressing.rs returns `reached_emulator_endpoint(parts)` (the Host header / absolute-form authority) first, falling back to `served_endpoint_authority(served_base)`, and only reaching the AWS-shaped `endpoint_for(region, device)` derivation when the emulator is served on no port at all. Any ordinary client — which reaches the emulator by its own host:port — therefore always gets the loopback authority back. Note the related FIXED issue 20261111T010133Z-iotdeviceadvisor-getendpoint-returns-the-account-level-endpoint-for.md: at that commit GetEndpoint still vended an AWS-shaped `<label>.deviceadvisor.iot.<region>.amazonaws.com` host and only the leading label was wrong; the call here is a 200, not a refusal, so this is not a refusal left behind by that fix, but the address-announcing change it introduced is what now puts `endpoint` outside its modeled bound.

Why: a client relying on AWS's behaviour gets two things wrong. First, any SDK or validator that enforces the modeled `@length 45..75` on `Endpoint` rejects the 200 response outright, so the call fails client-side even though the service answered. Second, a client that parses the endpoint — to pick the region a device under test should connect in, to build the `<label>.deviceadvisor.iot.<region>.amazonaws.com` name, or to distinguish the account-level host from a device-level host — finds neither a region nor a Device Advisor label, so provisioning a device from the returned value is impossible and every call collapses onto one indistinguishable loopback authority.

Fix: the endpoint derivation this operation delegates to — `announced_endpoint` in src/state/endpoint_addressing.rs — and the `GetEndpoint` handler that calls it. The vended value must be the AWS-shaped Device Advisor hostname produced by `endpoint_for(region, device)` (hostname only, 45..75 chars, region embedded, under the Device Advisor domain), not the reached/served authority; if the emulator also needs to publish a dialable local address, that belongs in addressing/routing for the data plane, not in this modeled member. `StartSuiteRun` reports its run's endpoint from the same derivation, so the two reports stay consistent once it is fixed there.

Traces: behavioral check iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain (GROUP control-plane, tags axis:happy-path / cap:c5 / concern:representation / plane:control).

## Evidence
Failing check: iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain (verdict: mismatch).

Check body:
    resp = da.get_endpoint()
    endpoint = resp["endpoint"]
    assert 45 <= len(endpoint) <= 75
    assert re.fullmatch(r"[A-Za-z0-9.-]+(:\\d+)?", endpoint)
    assert ctx.region in endpoint
    assert "deviceadvisor" in endpoint or "devicegateway" in endpoint

Expected (AWS): HTTP 200, body `{"endpoint": "<label>.deviceadvisor.iot.<region>.amazonaws.com"}` — a DNS hostname 45..75 characters long (the modeled `Endpoint` `@length` bound), containing the request's region and the Device Advisor domain. The concrete label, region and account id are per-environment values, not part of the requirement.

Actual (emulator): HTTP 200, body `{"endpoint": "127.0.0.1:32893"}`.
Replay output: `AssertionError: expected endpoint length in [45, 75], got 15: '127.0.0.1:32893'` (raised at the first assertion; the region and `deviceadvisor`/`devicegateway` assertions also do not hold for this value). The specific IP and port are per-run values; the defect is that a bare reached-authority is vended at all instead of a Device Advisor hostname.

Status and error code are not in question — the operation answered 200 as modeled. The divergence is solely the value/shape of the single modeled output member `endpoint`.

Code: src/ops/get_endpoint.rs lines 199-212 call `crate::state::announced_endpoint(ctx.parts(), ctx.endpoint().as_str(), &region, &device)`; src/state/endpoint_addressing.rs lines 44-53 resolve that as `reached_emulator_endpoint(parts).or_else(|| served_endpoint_authority(served_base)).unwrap_or_else(|| endpoint_for(region, device))`, so the AWS-shaped derivation is only reached when the emulator is served on no port.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay behavioral check `iotdeviceadvisor.get-endpoint-returns-valid-device-advisor-domain`.
3. Equivalently by hand: with a botocore client for `iotdeviceadvisor` pointed at the emulator, call `get_endpoint()` with no arguments (GET /endpoint, no query string), signed for any region.
4. Observe the 200 response body's `endpoint`: it is the `host:port` the client dialed the emulator on (e.g. `127.0.0.1:<port>`), 15 characters, with no region substring and no `deviceadvisor`/`devicegateway` label.
5. Expected: a 200 whose `endpoint` is a Device Advisor DNS hostname — 45..75 characters (the modeled `Endpoint` length bound), matching `[A-Za-z0-9.-]+`, containing the signing region, and ending in the Device Advisor domain `.deviceadvisor.iot.<region>.amazonaws.com`.
