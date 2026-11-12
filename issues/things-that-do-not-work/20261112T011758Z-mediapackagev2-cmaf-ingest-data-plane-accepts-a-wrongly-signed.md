Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.cmaf-ingest-native-protocol-round-trip
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.cmaf-ingest-native-protocol-round-trip|run#24|AssertionError|assert not (200 <= signed_status < 300), (
Issue-Commit: 06427132ab34f0591e7437b64f91b88a57797a82

# CMAF ingest data plane accepts a wrongly-signed SigV4 PUT with HTTP 200 and an empty body instead of a 403 signature-verification denial

> _Note: this issue was identified against the code base at commit `06427132ab34f0591e7437b64f91b88a57797a82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Behavioral replay of simparity-behavioral-aws-v1 (65/68 passed) leaves one mediapackagev2 failure that is neither marked ALREADY FILED nor OWNED by any open issue's replay key: the channel ingest listener now exists and now denies an unauthenticated PUT correctly, but it does not verify the SigV4 Authorization header it is handed — a signed PUT is ACCEPTED with HTTP 200 and an empty body. This is a ONE-cause bundle.

Related issues, named explicitly because none of them covers this check: 20261110T042138Z-mediapackagev2-data-plane-urls-channel-ingest-and-origin.md (open) carries this check id in its trace list, but its replay key is bound to the PublishedUrlRejected failure at `ingest_url = ctx.endpoint_url(base_url)` — the advertised ingest host is now reachable, the check now gets past that guard, and it fails later, at the signed-PUT assertion; that key therefore does not gate this defect. 20261023T045024Z-mediapackagev2-data-plane-does-not-enforce-authorization-unauthenticated.md and 20261029T201731Z-mediapackagev2-five-rejection-messages-worded-differently-from-aws.md (both fixed) covered the unauthenticated/un-granted classes answering 404 instead of 403; that class now passes. This finding is the second, distinct authorization class — a request that DOES carry an Authorization header — and it is not a refusal regression of either fixed issue, it is a success where AWS refuses.

Cause 1 of 1: the ingest listener does not verify the SigV4 signature on a PUT, so a cryptographically invalid signature is treated as success.
  AWS: Smithy mediapackagev2 CreateChannel -> POST /channelGroup/{ChannelGroupName}/channel, output member IngestEndpoints (IngestEndpointList of IngestEndpoint{Id, Url}); IngestEndpoint.Url is "The ingest domain URL where the source stream should be sent." The model declares AccessDeniedException (httpError 403) on the service's operations, and the ingest data plane behind that URL is a SigV4-authenticated endpoint. Recorded behavior only for the data-plane statuses (the model does not describe the push protocol): the recording shows the live ingest listener answering a PUT with NO Authorization header with HTTP 403 and a body containing "Access denied", and a PUT carrying a syntactically well-formed but cryptographically wrong SigV4 Authorization header (signed for service name `mediapackagev2` in the request's region) with a DIFFERENT HTTP 403 whose body is signature-verification specific — it names the signature as the reason (the body contains "signature"). Never a 2xx for either class, and never the identical body for both. The fixed text of each denial is that class's message; any request id, resource name, credential scope or canonical-string fragment inside it is a value taken from the request and the emulator's state, not part of the requirement.
  Emulator: the unauthenticated class is now answered correctly, but the signed class is answered HTTP 200 with an empty body: `AssertionError: expected the live ingest endpoint to reject a wrongly-signed PUT, but got HTTP 200: b''`. The listener evidently routes on path only and returns success for any request that carries an Authorization header (or ignores the header entirely), performing no canonical-request reconstruction, no signature comparison and no credential-scope check. Consequently the second negative class, and the requirement that the two denial bodies differ, are not met; the bytes PUT (`b"not a valid CMAF fragment"`, not a CMAF fragment at all) are also accepted.
  Why: a client that relies on AWS's behavior cannot distinguish a correctly-signed contribution push from a mis-signed one against the emulator. Encoder credential/region/clock-skew misconfiguration — the single most common ingest failure in production — appears to succeed, so test suites that assert a bad signature is rejected pass against the emulator and fail against AWS; and the emulator's ingest plane is effectively unauthenticated for anyone who sends any Authorization header, which is the opposite of the 403 contract the control plane's advertised ingest URL implies.
  Fix: the ingest data-plane listener's authorization stage for PUT on the channel ingest path (`/in/v1/<group>/<n>/<channel>/<...>`) — it must verify the presented SigV4 signature (recompute the canonical request over method, URI, query, signed headers and payload hash, and compare against the Authorization header's Signature for the credential scope/region/service) and, on mismatch, refuse with 403 and the signature-specific denial body, keeping the existing "Access denied" 403 for the header-absent class so the two bodies remain distinct. The success path must only be taken for a signature that verifies.
  Traces: mediapackagev2.cmaf-ingest-native-protocol-round-trip.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: Total 65/68 passed (95.6%), mismatched 1, errored 2, skipped 0, unloadable 0, bootstrap_signal false. Per-service: mediapackagev2 65/66 (98%), 1 failed; cloudformation__mediapackagev2 0/2, both OWNED by 20261109T223435Z-cloudformation-mediapackagev2-cloudformation-does-not-register-any-aws.md and skipped here.

Failing check mediapackagev2.cmaf-ingest-native-protocol-round-trip, group native-protocol, verdict [mismatch]:
  Sequence: CreateChannelGroup(g) -> CreateChannel(g, c, InputType='HLS') -> read IngestEndpoints[0].Url (now a host the emulator answers on, so the published-URL guard no longer refuses it) -> PUT b'not a valid CMAF fragment' with no Authorization header (PASSES: 403 with an 'access denied' body) -> PUT the same bytes with an Authorization header produced by botocore SigV4Auth(frozen creds, 'mediapackagev2', region).
  Assertion that failed (check line 63): `assert not (200 <= signed_status < 300)` ->
  `AssertionError: expected the live ingest endpoint to reject a wrongly-signed PUT, but got HTTP 200: b''`
  Expected: HTTP 403, a body whose text is signature-verification specific (contains "signature"), and a body DIFFERENT from the unauthenticated 403 body. Actual: HTTP 200, body b''.
  The three assertions after it (status == 403, body mentions the signature, body differs from the unauthenticated denial) are consequently unreached.
  Per-run values in this trace — the generated group/channel names, the emulator host/port, the ingest label and account label embedded in the URL, the credential values and the signature string — are NOT part of the requirement; the requirement is that a signature that does not verify is answered 403 with a signature-specific denial distinct from the no-Authorization denial.

## Steps to Reproduce
Boot the emulator from this checkout and replay mediapackagev2.cmaf-ingest-native-protocol-round-trip, or reproduce directly:
  1. mp = boto3.client('mediapackagev2', endpoint_url=<emulator endpoint>)
  2. mp.create_channel_group(ChannelGroupName=g)
  3. created = mp.create_channel(ChannelGroupName=g, ChannelName=c, InputType='HLS')
  4. url = created['IngestEndpoints'][0]['Url']  (resolve it to the emulator's own host/port as the harness does)
  5. PUT b'not a valid CMAF fragment' to url with NO Authorization header -> currently HTTP 403 with an 'Access denied' body (correct; keep this).
  6. Build an AWSRequest(method='PUT', url=url, data=b'not a valid CMAF fragment'), sign it with botocore SigV4Auth(frozen_credentials, 'mediapackagev2', region).add_auth(req), and PUT with dict(req.headers).
     Observed: HTTP 200, body b''.
     Expected: HTTP 403 with a signature-verification-specific body (mentioning the signature) that differs from the body returned in step 5.
  7. Also verify a request whose Authorization header is well-formed but whose Signature hex is altered by one character is refused the same way, and that a correctly-signed PUT is the only case taking the success path.
Pass criterion: no 2xx for any PUT whose SigV4 signature does not verify; two distinct 403 denial bodies for the header-absent and signature-invalid classes.
