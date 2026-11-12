Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-encoder-configuration-fractional-framerate-persists
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-encoder-configuration-fractional-framerate-persists ivs-realtime-encoder-configuration-omitted-name-defaults-to-empty-string ivs-realtime-ingest-configuration-created-inactive ivs-realtime-update-ingest-configuration-redundant-ingest-toggle ivs-realtime-tag-and-untag-resource-roundtrip
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|CreateEncoderConfiguration|2xx->2xx|$.encoderConfiguration.video.framerate
Issue-Key: ivs-realtime|GetEncoderConfiguration|2xx->2xx|$.encoderConfiguration.video.framerate
Issue-Key: ivs-realtime|CreateEncoderConfiguration|2xx->2xx|$.encoderConfiguration.name
Issue-Key: ivs-realtime|GetEncoderConfiguration|2xx->2xx|$.encoderConfiguration.name
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$.ingestConfiguration.userId
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.access-control-allow-origin
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.access-control-expose-headers
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.cache-control
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.content-security-policy
Issue-Key: ivs-realtime|GetIngestConfiguration|2xx->2xx|$.ingestConfiguration.userId
Issue-Key: ivs-realtime|ListIngestConfigurations|2xx->2xx|$.ingestConfigurations[*].userId
Issue-Key: ivs-realtime|TagResource|2xx->2xx|$headers.content-length
Issue-Key: ivs-realtime|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# ivs-realtime response projection drops documented empty-string defaults, mangles float framerate, CORS headers and 204 Content-Length (5 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEncoderConfiguration

## Description
Five independent response-serialization/projection defects in the ivs-realtime crate, all found by replaying the wire corpus (9 divergence clusters, every one a 200/204 whose BODY or HEADERS diverge). They share one fault family: the value the emulator puts on the wire is not the value AWS puts on the wire for the same resource state. A fixer must close all five before submitting.

Cause 1 of 5 — a float member is widened from f32 and serialized with f32 round-off.
AWS: Smithy: Video$framerate targets Framerate, type `float`, @range min 1 max 60 ("Video frame rate, in fps. Default: 30"). AWS serializes the configured value as the decimal the client sent, e.g. a request of 29.97 is echoed by CreateEncoderConfiguration and read back by GetEncoderConfiguration as 29.97.
Emulator: both operations answer 200 with $.encoderConfiguration.video.framerate = 29.969999313354492 — the f32 nearest to the requested value re-printed at f64 precision.
Why: a client that round-trips a configuration (read, modify one unrelated field, write back) sends a framerate that fails nothing but is no longer the value it configured, and any equality/assertion on the configured frame rate fails. Fractional broadcast rates (29.97, 59.94) are the normal case for this member.
Fix: the encoder-configuration response serializer (and whatever stores Video$framerate): keep/print the value with float (f32) formatting — shortest representation that round-trips as f32 — instead of widening the f32 to f64 before formatting.
Traces: ivs-realtime-encoder-configuration-fractional-framerate-persists#task#1 (CreateEncoderConfiguration), #task#2 (GetEncoderConfiguration).

Cause 2 of 5 — EncoderConfiguration$name is omitted instead of reported as the empty string when the create request omitted it.
AWS: Smithy: EncoderConfiguration$name targets EncoderConfigurationName, @length min 0 max 128, pattern ^[a-zA-Z0-9-_]*$, optional ("Optional name to identify the resource"). Recorded behavior: when CreateEncoderConfiguration is called with no `name`, AWS materializes it as the empty string and returns `"name": ""` on the create response and on every later read.
Emulator: $.encoderConfiguration.name is absent (null) on both the 200 create and the 200 get.
Why: a client that reads `.name` on the returned resource gets null/undefined instead of "", so strongly typed SDKs surface an absent optional where AWS guarantees a value, and name-based comparison or display logic diverges.
Fix: the encoder-configuration state/projection: materialize an omitted name as "" at create time so every projection emits the member.
Traces: ivs-realtime-encoder-configuration-omitted-name-defaults-to-empty-string#task#1 (CreateEncoderConfiguration), #task#2 (GetEncoderConfiguration).

Cause 3 of 5 — IngestConfiguration$userId is omitted instead of reported as the empty string.
AWS: Smithy: IngestConfiguration$userId targets UserId, @length min 0 max 128, optional ("Customer-assigned name to help identify the participant using the IngestConfiguration"). Recorded behavior: an ingest configuration created without a userId reports `"userId": ""` on the create response, on GetIngestConfiguration, inside ListIngestConfigurations items and on UpdateIngestConfiguration.
Emulator: $.ingestConfiguration.userId (and $.ingestConfigurations[N].userId in the list projection) is absent on every one of those paths.
Why: same client consequence as cause 2, on a member AWS always emits — the list projection and the single-resource projection both disagree with AWS, so a client cannot distinguish "no user id" from "field not served".
Fix: the ingest-configuration state/projection shared by Create/Get/Update/List: default an absent userId to "" and always emit the member. This is the same empty-string-default class already noted for layout members in open issue 20261028T173549Z-ivs-realtime-startcomposition-wrong-behavior--r3 ("Layout members whose documentation states `Default: \"\"` are reported absent"); that issue is scoped to composition layout members and does not cover EncoderConfiguration$name or IngestConfiguration$userId.
Traces: ivs-realtime-ingest-configuration-created-inactive#task#2 (CreateIngestConfiguration), #task#3 (GetIngestConfiguration), #task#5 (ListIngestConfigurations). The same member is also absent on ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2..#task#6 (bundled there with the redundantIngest cause).

Cause 4 of 5 — the modeled CORS/security response headers carry the wrong values and one is not emitted at all.
AWS: Smithy: CreateIngestConfiguration/UpdateIngestConfiguration outputs bind accessControlAllowOrigin -> @httpHeader("Access-Control-Allow-Origin"), accessControlExposeHeaders -> "Access-Control-Expose-Headers", cacheControl -> "Cache-Control", contentSecurityPolicy -> "Content-Security-Policy" (plus strictTransportSecurity, xContentTypeOptions, xFrameOptions). Recorded behavior on a 200: Access-Control-Allow-Origin: *; Cache-Control: no-store, no-cache; Content-Security-Policy: default-src 'self'; upgrade-insecure-requests;; Access-Control-Expose-Headers: the service's full fixed list (Access-Control-Allow-Origin,Access-Control-Expose-Headers,Cache-Control,Content-Length,Content-Security-Policy,Content-Type,date,Strict-Transport-Security,x-amz-apigw-id,x-amzn-errormessage,x-amzn-errortype,x-amzn-requestid,x-amzn-trace-id,X-Content-Type-Options,X-Frame-Options). These are fixed strings, not per-request values.
Emulator: Access-Control-Allow-Origin is missing; Content-Security-Policy is "default-src 'none'"; Cache-Control is "no-store"; Access-Control-Expose-Headers is the crate's own short lowercase list "x-amzn-errortype, x-amzn-requestid, x-amzn-errormessage, x-amzn-trace-id, x-amz-apigw-id, date".
Why: a browser-based client is the whole point of these headers — with no Access-Control-Allow-Origin the response is rejected by the user agent, and with the truncated expose-headers list the client cannot read Content-Type/Content-Length/Content-Security-Policy off the response. The earlier resolved issue 20261024T135513Z-ivs-realtime-createingestconfiguration-wrong-behavior made the crate emit some of these members; it does not cover these four values.
Fix: the ingest-configuration response serializer / the crate's common header layer: populate the modeled @httpHeader output members with the service's fixed strings rather than the framework's generic defaults.
Traces: ivs-realtime-ingest-configuration-created-inactive#task#2 (CreateIngestConfiguration; cluster also covers ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2).

Cause 5 of 5 — the empty-body 204 of TagResource/UntagResource omits Content-Length.
AWS: Smithy: TagResource (POST /tags/{resourceArn}) and UntagResource (DELETE /tags/{resourceArn}) both have an empty output structure. Recorded behavior only: the recorded responses are 204 with no body and DO carry a Content-Length header (the corpus asserts the header's presence/shape, not a literal value).
Emulator: status 204 matches, but no Content-Length header is emitted on either operation.
Why: a client or proxy that keys off Content-Length to decide the response is complete (and the Access-Control-Expose-Headers list above names Content-Length) sees a header AWS always sends go missing, which changes framing behavior for HTTP intermediaries.
Fix: the tagging operations' response serializer / the crate's empty-body response path: emit Content-Length for the empty body as the recorded responses do.
Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#2 (TagResource), #task#4 (UntagResource).

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 56 steps, 40 passed, 16 mismatched, score 71.4%; emulator up, /__admin/health 200). 9 clusters, all verdict=mismatch, all expected==actual status:

1. ivs-realtime-encoder-configuration-fractional-framerate-persists#task#1 (CreateEncoderConfiguration, 200->200): $.encoderConfiguration.video.framerate: expected 29.97, got 29.969999313354492.
2. ...#task#2 (GetEncoderConfiguration, 200->200): same, expected 29.97, got 29.969999313354492.
3. ivs-realtime-encoder-configuration-omitted-name-defaults-to-empty-string#task#1 (CreateEncoderConfiguration, 200->200): $.encoderConfiguration.name: missing (expected '', got None).
4. ...#task#2 (GetEncoderConfiguration, 200->200): $.encoderConfiguration.name: missing (expected '', got None).
5. ivs-realtime-ingest-configuration-created-inactive#task#2 (CreateIngestConfiguration, 200->200): $.ingestConfiguration.userId: missing (expected '', got None); $headers.access-control-allow-origin: missing (expected '*', got None); $headers.content-security-policy: expected "default-src 'self'; upgrade-insecure-requests;", got "default-src 'none'"; $headers.cache-control: expected 'no-store, no-cache', got 'no-store'; $headers.access-control-expose-headers: expected 'Access-Control-Allow-Origin,Access-Control-Expose-Headers,Cache-Control,Content-Length,Content-Security-Policy,Content-Type,date,Strict-Transport-Security,x-amz-apigw-id,x-amzn-errormessage,x-amzn-errortype,x-amzn-requestid,x-amzn-trace-id,X-Content-Type-Options,X-Frame-Options', got 'x-amzn-errortype, x-amzn-requestid, x-amzn-errormessage, x-amzn-trace-id, x-amz-apigw-id, date' (cluster also covers ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2).
6. ivs-realtime-ingest-configuration-created-inactive#task#3 (GetIngestConfiguration, 200->200): $.ingestConfiguration.userId: missing (expected '', got None).
7. ...#task#5 (ListIngestConfigurations, 200->200): $.ingestConfigurations[0].userId: missing (expected '', got None).
8. ivs-realtime-tag-and-untag-resource-roundtrip#task#2 (TagResource, 204->204): $headers.content-length: missing (expected present/shaped, got None).
9. ...#task#4 (UntagResource, 204->204): $headers.content-length: missing (expected present/shaped, got None).

No cluster carried stub_path or owned_by, so none of this is covered by an existing issue or a stub placeholder.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /CreateEncoderConfiguration with video.framerate = 29.97, then GetEncoderConfiguration on the returned ARN — fixture ivs-realtime-encoder-configuration-fractional-framerate-persists, steps task#1 and task#2. Observe 29.969999313354492 in both responses.
3. Causes 2: POST /CreateEncoderConfiguration with no `name`, then GetEncoderConfiguration — fixture ivs-realtime-encoder-configuration-omitted-name-defaults-to-empty-string, steps task#1/task#2. Observe `name` absent instead of "".
4. Causes 3 and 4: POST /CreateIngestConfiguration without userId, then GetIngestConfiguration and ListIngestConfigurations — fixture ivs-realtime-ingest-configuration-created-inactive, steps task#2, task#3, task#5. Observe `userId` absent in all three projections, and on the create response inspect the response headers: no Access-Control-Allow-Origin, and Content-Security-Policy / Cache-Control / Access-Control-Expose-Headers carry the crate's generic values.
5. Cause 5: POST /tags/{resourceArn} then DELETE /tags/{resourceArn}?tagKeys=... — fixture ivs-realtime-tag-and-untag-resource-roundtrip, steps task#2 and task#4. Observe 204 with no Content-Length header.

## Resolution

Causes 1, 2, 3 and 4 are fixed. Eleven of the thirteen `Issue-Key` causes replay clean:
both `video.framerate` keys (a `float` member is rendered from its own binary32 value,
`src/ops/types/float_wire.rs`), both `encoderConfiguration.name` keys and every
`ingestConfiguration.userId` key (both members are materialised at creation and reported by
every projection), and all four header keys of cause 4 — the seven CORS and security
members those output shapes bind are now one statement of the endpoint's own fixed values
(`crate::state::RESPONSE_HEADER_POLICY`), including the cross-origin read grant and the
content policy that travels with it, served on every response whatever the outcome of the
call.

Two keys remain divergent, and they are a platform limit rather than unimplemented
behaviour: `$headers.content-length` on TagResource and UntagResource. The service DOES
state the length of its empty 204 (`src/service.rs`, pinned in
`tests/all/empty_response_framing.rs` and `tests/all/tagging.rs`); the HTTP writer under the
vendored transport removes it, because RFC 9110 forbids `content-length` on a 204 —
`vendor/server/src/lib.rs` states that rule for the in-process path too, and
`tests/all/wire_probe.rs` pins the boundary with a real socket. Nothing a service crate can
change puts the header on a socket response.
