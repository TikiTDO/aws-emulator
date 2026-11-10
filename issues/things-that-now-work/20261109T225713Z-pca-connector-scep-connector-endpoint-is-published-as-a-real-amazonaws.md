Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip|_fetch#12|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(f"{endpoint}?operation=GetCACert")
Issue-Commit: 6bb51c191bdb333bc331bcde4b456674734a5a8d

# Connector.Endpoint is published as a real amazonaws.com SCEP host, so the connector's SCEP data plane is unreachable from any client of the emulator

> _Note: this issue was identified against the code base at commit `6bb51c191bdb333bc331bcde4b456674734a5a8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnector

## Description
1 net-new root cause.

Related open issues on the same surface, NEITHER of which the replay marked as owning this check (their recorded symptoms are different: 20261103T120430Z is a 60s/90s poll timeout with no emulator response quoted, and 20261107T164213Z is a 404 `UnknownService` returned after the request was actually routed to the emulator): 20261103T120430Z-pca-connector-scep-connector-endpoint-serves-no-scep-native-protocol-getcacert.md and 20261107T164213Z-pca-connector-scep-connector-endpoint-host-is-not-routed-at-all.md. This filing records a cause that sits EARLIER in the same flow and is now the first thing that fails: the request is no longer even attempted, because the address the emulator publishes in `Connector.Endpoint` is not an address of the emulator. Those two issues are about SERVING the SCEP protocol once a request arrives; this one is about ANNOUNCING an address a client can reach. If they are fixed, a connector endpoint on an amazonaws.com host would still be unreachable, so this must be closed too; if they are still open, all three should be closed together.

Cause 1 of 1: an ACTIVE connector's `Connector.Endpoint` is a synthesized real-AWS hostname of the form `https://<connector-id>.scep.pca-connector-scep.<region>.amazonaws.com`, i.e. the production DNS name, rather than an address served by the emulator itself. No request to that host can reach the emulator — it resolves (or fails to resolve) out on the public Internet, not at the emulator's listener — so the connector's whole SCEP data plane (GetCACert, GetCACaps, PKIOperation) is unreachable no matter what the emulator serves internally. The behavioral harness refuses the URL before issuing the GET, with `simparity.published.PublishedUrlRejected: endpoint_url was given https://<connector-id>.scep.pca-connector-scep.us-west-2.amazonaws.com?operation=GetCACert (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. Failing check id: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip.

AWS: Smithy: `com.amazonaws.pcaconnectorscep#Connector$Endpoint` (target `String`) — "The connector's HTTPS public SCEP URL" (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json). The SCEP operations themselves are NOT modeled operations of pca-connector-scep, so for the protocol this is recorded behavior only: the value in `Connector.Endpoint` is an HTTPS URL that the caller can actually fetch, and `GET <Endpoint>?operation=GetCACert` against an ACTIVE connector answers HTTP 200 with a non-empty body — a single DER X.509 certificate (Content-Type `application/x-x509-ca-cert`) or a DER PKCS#7 certificates-only structure (Content-Type `application/x-x509-ca-ra-cert`) — containing the certificate of the private CA named by the connector's `CertificateAuthorityArn` (same subject, same serial number). The requirement is that `Endpoint` is a URL reachable by the caller at the same service it just called, and that a GetCACert GET to it returns the configured CA certificate. The concrete host, the connector id inside it, the region and the certificate's subject/serial are per-run values and are not part of the requirement.

Emulator: the control plane completes — CreateConnector returns 202 with a ConnectorArn and GetConnector settles the connector to ACTIVE — but the `Connector.Endpoint` it reports is the production AWS SCEP hostname pattern `https://<connector-uuid>.scep.pca-connector-scep.<region>.amazonaws.com` (the recorded run shows connector id `395833ce-1369-4d43-bcab-e12a2d54b082` and region `us-west-2`; the fixed part is the `<id>.scep.pca-connector-scep.<region>.amazonaws.com` template, the id and region are per-run values from the request and state). The emulator is reached on its own host and port, and it announces a host that is not one of its own, so the announced endpoint is dead for every client: the harness's published-URL check rejects it outright (`PublishedUrlRejected`, "its host is not one of the emulator's own hosts") and the check never gets as far as sending the SCEP GET, where the previously filed 404 `UnknownService` was observed.

Why: a client that follows the documented flow — create and activate a private CA, share it with the pca-connector-scep service principal via RAM, CreateConnector, read `Connector.Endpoint`, then point a SCEP-enabled device or an MDM (Intune, Jamf) at that URL — is handed an address that does not belong to the emulator. Against the emulator the fetch either fails to resolve, times out, or (worse) escapes to the real Internet and talks to something other than the emulator; nothing in the response is the emulator's. Every SCEP enrollment path is therefore untestable, and the failure surfaces as a network/DNS error rather than as any AWS error, so client code that distinguishes AWS faults from transport faults mis-classifies it and retry loops spin to their own deadlines. It also hides the downstream defects: the SCEP handler and its routing can never be exercised while the advertised address points elsewhere.

Fix: the connector's endpoint-address construction in the pcaconnectorscep crate — the state/model code that computes and stores `Connector.Endpoint` when CreateConnector provisions a connector, and the serializer for `GetConnector`/`ListConnectors` that returns it. It must publish an address on the emulator's own published host and port (the same base the caller reached the control plane on, or a per-resource listener the emulator owns), carrying the connector id in the path or in a host label of a host the emulator actually serves, so that `<Endpoint>?operation=GetCACert` is routed back into this emulator. The host must never be hard-coded to `amazonaws.com`; it must come from the emulator's published-endpoint configuration. The SCEP handler and the host-to-service routing that must then answer that request are the subject of the two related issues named above.

Traces: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (mismatch, `simparity.published.PublishedUrlRejected: endpoint_url was given https://395833ce-1369-4d43-bcab-e12a2d54b082.scep.pca-connector-scep.us-west-2.amazonaws.com?operation=GetCACert ... its host is not one of the emulator's own hosts`).

## Evidence
Check: pca-connector-scep.scep-get-cacert-native-protocol-roundtrip (GROUP native-protocol, tags axis:happy-path, cap:c1, cap:c4, concern:data-plane, dp:endpoint, plane:data). Suite service heading: ### pca-connector-scep. Suite run: 12/21 passed (57.1%), 6 mismatched / 3 errored; this is the ONLY failing check in the run not marked ALREADY FILED or OWNED.

Expected (AWS): after CreateConnector against an ACTIVE, RAM-shared private CA, `GetConnector(ConnectorArn=...)["Connector"]["Endpoint"]` is an HTTPS SCEP URL the caller can fetch at the same service endpoint it just called, and `GET <Endpoint>?operation=GetCACert` answers HTTP 200 with a non-empty DER X.509 or DER PKCS#7 body containing the CA certificate of the connector's `CertificateAuthorityArn` (same subject and serial number).

Actual (emulator): CreateConnector answers 202 with e.g.

    {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'ConnectorArn': 'arn:aws:pca-connector-scep:us-west-2:000000000000:connector/...'}

the connector reaches ACTIVE, and `Connector.Endpoint` is

    https://395833ce-1369-4d43-bcab-e12a2d54b082.scep.pca-connector-scep.us-west-2.amazonaws.com

Routing that URL for the client fails before any request is sent:

    File "context.py", line 572, in endpoint_route
        return self._published_target(url, "endpoint_url"), {}
    File "context.py", line 586, in _published_target
        raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
    simparity.published.PublishedUrlRejected: endpoint_url was given
    https://395833ce-1369-4d43-bcab-e12a2d54b082.scep.pca-connector-scep.us-west-2.amazonaws.com?operation=GetCACert
    (published by the emulator or built by the check): its host is not one of the emulator's own hosts;
    a customer of the emulator needs http(s)://127.0.0.1:35059/<resource path>, or
    http(s)://<own host>:<port>/<path> on a per-resource listener

Message/value anatomy: the diverging value is the published `Connector.Endpoint`. Its fixed, template part is `https://<connector-id>.scep.pca-connector-scep.<region>.amazonaws.com` — a production AWS DNS name. Its variable parts are the connector id (from the CreateConnector just performed) and the region (from the request); the host:port shown in the rejection text (127.0.0.1:35059) is this run's emulator address and is itself a per-run value, not a requirement. The divergence is that the announced host is not a host the emulator serves, so no HTTP status, error code or body is produced at all for the SCEP surface.

Model citation used: `com.amazonaws.pcaconnectorscep#Connector$Endpoint` — "The connector's HTTPS public SCEP URL". SCEP's GetCACert is not a modeled operation, so the protocol expectation is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2 in the recording; any region reproduces).
2. acm-pca: CreateCertificateAuthority (ROOT, RSA_2048 / SHA256WITHRSA), wait for PENDING_CERTIFICATE, GetCertificateAuthorityCsr, IssueCertificate with TemplateArn arn:aws:acm-pca:::template/RootCACertificate/V1 and Validity 10 YEARS, GetCertificate, ImportCertificateAuthorityCertificate, wait for ACTIVE. Keep the returned CA certificate PEM.
3. ram: CreateResourceShare for that CA ARN with principal pca-connector-scep.amazonaws.com and permission arn:aws:ram::aws:permission/AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority; wait until the share is ACTIVE.
4. pca-connector-scep: CreateConnector(CertificateAuthorityArn=<ca arn>); poll GetConnector until Connector.Status == ACTIVE and read Connector.Endpoint.
5. Inspect Connector.Endpoint: it is https://<connector-id>.scep.pca-connector-scep.<region>.amazonaws.com — a public AWS hostname, not the host:port the emulator is being reached on.
6. Try to fetch `<Endpoint>?operation=GetCACert` as a client of the emulator. The address does not belong to the emulator, so the request cannot reach it (the harness rejects the published URL with PublishedUrlRejected: "its host is not one of the emulator's own hosts"). Expected: Endpoint is an address on the emulator's own published host/port, and the GET answers HTTP 200 whose DER or PKCS#7 body contains the CA certificate from step 2 (same subject and serial number).
