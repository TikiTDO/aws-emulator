Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected iot.iot-mtls-retained-publish-rejected-without-retainpublish-action iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition iot.shadow-client-token-echoed-not-persisted
Issue-Key: iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected|run#14|AssertionError|assert status == 200, (
Issue-Key: iot.iot-mtls-retained-publish-rejected-without-retainpublish-action|run#12|AssertionError|assert status == 200, (
Issue-Key: iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition|run#22|AssertionError|assert status == 403, (
Issue-Key: iot.shadow-client-token-echoed-not-persisted|_iot_data#2|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", ep) and "-ats.iot." in ep, (
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# iot: the published data-plane address serves no certificate-authenticated (mTLS) HTTPS publish — POST /topics/{topic} answers 404 UnknownService — and endpointAddress is a bare host:port, not an ATS data-plane hostname (2 causes, 4 checks)

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Two net-new root causes in the iot service's data-plane discovery and routing. Both are the aftermath of the now-fixed issue 20261109T113725Z-iot-describeendpoint-publishes-a-real-aws-hostname-id.md (which made DescribeEndpoint publish the emulator's own listener instead of an amazonaws.com host); neither is covered by it or by the still-open 20261109T114414Z-iot-describeendpoint-announces-a-real-amazonaws-com-data.md / 20261109T120740Z-iot-the-published-data-plane-address-is-a.md / 20261109T122953Z-iot-3-net-new-causes-describeendpoint-publishes-real.md, which all describe the OLD behavior (a real-AWS host) and carry none of the replay keys below.

Cause 1 of 2: REOPENS 20261109T113725Z-iot-describeendpoint-publishes-a-real-aws-hostname-id.md: its fix left the call refused. The address DescribeEndpoint now publishes is the emulator's own front door, but a certificate-authenticated (mTLS) HTTPS publish to it is not routed to the IoT data plane at all.
AWS: Smithy com.amazonaws.iotdataplane#Publish — POST /topics/{topic}, topic as @httpLabel, qos/retain/contentType/responseTopic/messageExpiry as @httpQuery, payload as @httpPayload, success code 200 with an empty body; modeled errors UnauthorizedException (401), InvalidRequestException (400), MethodNotAllowedException (405), ThrottlingException (429), InternalFailureException (500). On AWS this request is accepted on the account's data endpoint over mutual TLS with a registered X.509 client certificate and NO SigV4 signature: the certificate is the principal and the attached IoT policy decides it. An allowed publish is 200; a publish the attached IoT policy does not permit (no matching iot:Publish, an unsatisfied Condition such as aws:SourceVpce, or a retained publish without iot:RetainPublish) is HTTP 403 whose body is exactly {"message": null, "traceId": "<uuid>"} — the two keys and the null message are fixed text, the traceId is a per-request UUID and is not part of the requirement.
Emulator: every such request answers HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The front door resolves a request to a service only from its SigV4 credential scope, X-Amz-Target or Host header (see the handler's own note that the published authority "resolves a request to the service its SigV4 credential scope names"), so a cert-authenticated data-plane request — which has none of the three — is rejected by the router before any IoT handler sees it. Neither the 200 nor the 403 half of the behavior is reachable, so no IoT-policy evaluation of a certificate principal (Condition keys, iot:Publish vs iot:RetainPublish) can be observed, and nothing is retained.
Why: a device doing exactly what AWS prescribes — DescribeEndpoint, then an mTLS POST /topics/<topic> with its registered certificate — gets 404 UnknownService with an error code that exists in no IoT model, instead of 200 or a 403 refusal. A client cannot distinguish "denied by policy" from "no such service", authorization tests pass vacuously, and the entire certificate-principal data plane is unreachable.
Fix: the front door's service resolution plus the iot-data plane's authentication — the router must dispatch the modeled iot-data HTTP paths (POST /topics/{topic}, and the /connections/* family) by path/listener when no SigV4 scope is present, and the iot-data Publish handler must authenticate the TLS client certificate as the IoT certificate principal and evaluate its attached IoT policies, returning 200 on allow and the modeled 403 body on deny.
Traces: iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected, iot.iot-mtls-retained-publish-rejected-without-retainpublish-action, iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition. Published address: services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs lines 176-200 (published_endpoint_address -> emulator_endpoint_authority).

Cause 2 of 2: DescribeEndpoint(iot:Data-ATS) returns a bare IP-literal-and-port authority instead of a data-plane HOSTNAME of the ATS form, so a client that parses the address (or uses it as a TLS/SNI host) has nothing AWS-shaped to work with.
AWS: Smithy com.amazonaws.iot#DescribeEndpoint — GET /endpoint, output member endpointAddress, whose documentation states "The endpoint. The format of the endpoint is as follows: identifier.iot.region.amazonaws.com."; for endpointType iot:Data-ATS the address is a DNS host name carrying the ATS marker, of the form <identifier>-ats.iot.<region>.<suffix> — a name, with no port component. The identifier, region and suffix are environment values; the shape (a dotted host name containing "-ats.iot.") is the requirement.
Emulator: endpointAddress is an authority with no host name structure at all — observed as a loopback address plus the front door's port (the specific address and port are per-run values and are not the defect; the absence of any "-ats.iot." host-name form is). A client that asserts the documented data-endpoint form rejects it, so the shadow data-plane checks stop at discovery and never exercise UpdateThingShadow/GetThingShadow.
Why: the documented discovery contract is "call DescribeEndpoint, then address the data plane at that host name". Code that validates or derives anything from the published form (the ATS marker, the embedded region, use of the name as the TLS SNI host for an MQTT or HTTPS session) breaks against a bare host:port, even though the authority happens to be reachable. The two requirements compose: the published value must be a host name of the ATS form AND resolve/route to the emulator's own listener.
Fix: the endpoint-address rendering in DescribeEndpoint's handler — publish a host name of the ATS data-plane form that the emulator also owns and routes (an own-host DNS name carrying the "-ats.iot.<region>" labels, with the port only where the listener needs one), rather than the raw front-door authority.
Traces: iot.shadow-client-token-echoed-not-persisted. Rendering site: services/service-com-amazonaws-iot-awsiotservice/src/ops/describe_endpoint.rs lines 176-200.

## Evidence
Cause 1 — iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected:
  Expected: the control principal's mTLS POST /topics/<topic> (policy allows iot:Connect/Publish/Subscribe/Receive/RetainPublish, no Condition) -> HTTP 200, empty body, message retained; the pinned principal's identical publish -> HTTP 403 with body {"message": null, "traceId": "<uuid>"} and nothing retained.
  Actual: AssertionError: the CONTROL principal (same document, no Condition) was refused with HTTP 404 (b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}') publishing to 'sim2real/scn/<topic>'.

  iot.iot-mtls-retained-publish-rejected-without-retainpublish-action:
  Expected: a plain publish with exactly ['iot:Connect','iot:Publish','iot:Subscribe','iot:Receive'] -> 200; the same publish with retain=true -> 403 with body keys exactly {message, traceId}, message null.
  Actual: HTTP 404 {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} on the plain publish.

  iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition:
  Expected: the certificate whose policy pins aws:SourceVpce -> 403 (IoT policy in force for the certificate principal), while the account's SigV4 caller publishing the same topic -> 200 (IoT policies are never consulted for SigV4 requests).
  Actual: the mTLS publish returned HTTP 404 {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}.
  Fixed text of the divergence: the status 404, the error code "UnknownService" and the quoted router message — none of which is an iot-data modeled error; the topic names and host/port in the diagnostics are per-run values.

Cause 2 — iot.shadow-client-token-echoed-not-persisted:
  Expected: DescribeEndpoint(endpointType='iot:Data-ATS') -> 200 with endpointAddress a data-plane host name of the ATS form (matching [A-Za-z0-9.-]+ and containing "-ats.iot."), e.g. <identifier>-ats.iot.<region>.<suffix>.
  Actual: 200 with a bare loopback authority including a port; the check fails with AssertionError: DescribeEndpoint(iot:Data-ATS) must return an ATS data-plane host, got '<addr>:<port>' — so UpdateThingShadow/GetThingShadow (clientToken echoed but never persisted, server-computed delta in the stored document) are never exercised.

## Steps to Reproduce
Cause 1:
1. Boot the emulator. Call iot CreatePolicy with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iot:Connect","iot:Publish","iot:Subscribe","iot:Receive","iot:RetainPublish"],"Resource":"*"}]} and iot CreateKeysAndCertificate(setAsActive=true); AttachPolicy the policy to the certificate ARN.
2. Call iot DescribeEndpoint(endpointType='iot:Data-ATS') and take endpointAddress.
3. Open a TLS connection to that address with the certificate and private key as the client certificate (no SigV4 signature) and send POST /topics/sim2real/scn/<x> with a JSON body.
4. Observe HTTP 404 {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} instead of 200.
5. Repeat with retain=true, and with a policy whose statement carries Condition {"StringEquals":{"aws:SourceVpce":"vpce-..."}}: both must be 403 with body {"message":null,"traceId":"<uuid>"}; both answer the same 404.
6. Replay iot.iot-mtls-publish-refused-when-policy-pins-sourcevpce-rejected, iot.iot-mtls-retained-publish-rejected-without-retainpublish-action, iot.iot-sigv4-publish-unaffected-by-certificate-policy-condition.

Cause 2:
1. Call iot DescribeEndpoint(endpointType='iot:Data-ATS').
2. Observe endpointAddress is a bare host:port with no "-ats.iot.<region>" host-name labels, so it does not match the documented identifier.iot.region form.
3. Replay iot.shadow-client-token-echoed-not-persisted: it stops at the discovery assertion before any shadow call.
