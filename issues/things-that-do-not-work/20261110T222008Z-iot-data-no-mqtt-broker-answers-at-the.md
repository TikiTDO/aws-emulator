Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.connected-client-connection-apis-reflect-live-session
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.connected-client-connection-apis-reflect-live-session|run#39|Timeout|raise AssertionError(
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# iot-data: no MQTT broker answers at the published data endpoint — an mTLS CONNECT never completes, so GetConnection / ListSubscriptions / SendDirectMessage can never reflect a live session (1 cause, 1 check)

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
Cause 1 of 1: the address DescribeEndpoint publishes is now the emulator's own front door, but nothing on it speaks MQTT, so a client's CONNECT never completes and the whole connection-management surface stays unobservable.
AWS: Smithy com.amazonaws.iotdataplane — GetConnection (GET /connections/{clientId}, includeSocketInformation as @httpQuery; output connected, clientId, thingName, cleanSession, sourceIp, sourcePort, targetIp, targetPort, keepAliveDuration, connectedSince, disconnectedSince, disconnectReason, sessionExpiry, vpcEndpointId; errors ForbiddenException 403, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429, InternalFailureException 500), ListSubscriptions (GET /connections/{clientId}/subscriptions, output subscriptions[].topicFilter, paginated) and SendDirectMessage (POST /connections/{clientId}/messages, topic as @httpQuery, payload as @httpPayload, output message + traceId). These three operations report and act on a LIVE MQTT session: AWS serves MQTT over TLS on port 8883 at the account's data endpoint, a certificate-authenticated client CONNECTs there, and from then on GetConnection reports connected=true with the open socket's sourceIp/sourcePort, ListSubscriptions names the topic filters that client SUBSCRIBEd, SendDirectMessage delivers the exact payload bytes to it on the requested topic, and after DISCONNECT GetConnection reports connected=false.
Emulator: a TLS MQTT CONNECT to the published data endpoint on the broker port never produces a CONNACK — the client waits and the check reaches the harness's 60 s cap ("Check timed out after 60.0s") without the session ever being established. No MQTT listener serves the published address, so no session state is created and none of the three connection operations can be exercised at all.
Why: a device that discovers its broker the documented way (DescribeEndpoint, then MQTT over TLS to that host on 8883) cannot connect to the emulator. Everything that depends on a live session is unreachable: connected-client reads, socket information, subscription listings, SendDirectMessage delivery, retained-message delivery on connect, and policy evaluation at CONNECT. A client gets an indefinite hang rather than any modeled response, which is worse than a refusal because it has no error to branch on.
Fix: the iot-data MQTT broker listener and its registration with the published endpoint — an MQTT-over-TLS listener must accept a certificate-authenticated CONNECT at the address iot DescribeEndpoint publishes, record the session (client id, socket, keep-alive, subscriptions), and the GetConnection / ListSubscriptions / SendDirectMessage handlers must read and act on that session state (including connected=false after DISCONNECT). This is the SAME check as open issue 20261109T120740Z-iot-data-the-mqtt-broker-address-published-by.md, but that issue's cause and replay key are the published host being a real-AWS ATS name rejected before any CONNECT (PublishedUrlRejected); the address is now the emulator's own and the failure has moved to there being no broker behind it, which that issue does not describe or cover.
Traces: iot-data.connected-client-connection-apis-reflect-live-session.

## Evidence
Expected: with a registered, policy-attached X.509 certificate, an MQTT-over-TLS CONNECT to the address from DescribeEndpoint(iot:Data-ATS) on the broker port (8883 on AWS) completes; then GetConnection(clientId, includeSocketInformation=true) -> 200 connected=true, clientId echoed, non-empty sourceIp and sourcePort > 0; ListSubscriptions(clientId) -> 200 whose subscriptions include the topicFilter the client SUBSCRIBEd; SendDirectMessage(clientId, topic, payload) -> 200 with a traceId AND the connected client receives exactly those payload bytes on that topic; after DISCONNECT, GetConnection -> connected=false.

Actual: the suite reports only "Error: Check timed out after 60.0s". The check's first wait is the CONNACK: it loops on the connect event until a 60 s deadline and would otherwise raise "DATAPLANE_UNIMPLEMENTED: MQTT never connected within 60s (last rc=...)" — the TCP/TLS connect itself did not raise OSError, so the address accepts a connection but never answers the MQTT CONNECT. None of GetConnection, ListSubscriptions or SendDirectMessage is reached, so none of their responses was observed.

## Steps to Reproduce
1. Boot the emulator. Call iot CreateKeysAndCertificate(setAsActive=true) and iot CreatePolicy with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:*","Resource":"*"}]}; AttachPolicy the policy to the certificate ARN.
2. Call iot DescribeEndpoint(endpointType='iot:Data-ATS') and take endpointAddress.
3. With paho-mqtt, tls_set(certfile=<cert>, keyfile=<key>) and connect to that endpoint host on the broker port (8883), then loop_start() and wait for on_connect.
4. Observe no CONNACK ever arrives (rc stays unset); the client hangs until the 60 s cap.
5. Consequently iot-data GetConnection(clientId, includeSocketInformation=true), ListSubscriptions(clientId) and SendDirectMessage(clientId, topic, payload) cannot be exercised against any live session.
6. Replay iot-data.connected-client-connection-apis-reflect-live-session.
