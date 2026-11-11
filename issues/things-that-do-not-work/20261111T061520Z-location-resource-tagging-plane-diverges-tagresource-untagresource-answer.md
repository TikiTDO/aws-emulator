Issue-Source: trace-sweep-divergence
Issue-Trace: location-geofence-tag-resource-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-geofence-tag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|TagResource|2xx->2xx|status
Issue-Key: location|UntagResource|2xx->2xx|status
Issue-Key: location|ListTagsForResource|4xx->4xx|$.message
Issue-Key: location|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 1456cf8869bc9f1924598db055521086c4da9a25

# Location resource-tagging plane diverges: TagResource/UntagResource answer 200 instead of 204, and ListTagsForResource not-found message is emulator-authored

> _Note: this issue was identified against the code base at commit `1456cf8869bc9f1924598db055521086c4da9a25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three causes, one shared fault: the location crate's shared resource-tagging plane (POST/DELETE/GET on /tags/{ResourceArn}) is wired up with the crate's generic empty-response and generic not-found behaviour instead of the service's own wire contract. The ARN resolution and tag storage work (the tag round-trip itself passes); what diverges is the status the two mutations answer with and the refusal body the read answers with.

Cause 1 of 3: TagResource answers HTTP 200 where the service answers 204.
AWS: Smithy: TagResource POST /tags/{ResourceArn}, output shape TagResourceResponse with NO members, and the operation carries no explicit @http code (the model reports code 200 as codeDefaulted, i.e. the model does not state a status for it). Recorded behavior only: a successful TagResource on an existing geofence collection answers HTTP 204 with an empty body. There is no response payload either way, so the status is the entire observable result of the call.
Emulator: answers HTTP 200 (body empty) for the same successful tagging call.
Why: the status code is the only signal this operation returns. A client, proxy, cache or test that branches on 204 No Content (or that asserts "no content" by status rather than by reading a zero-length body) takes the wrong branch, and any generated client whose deserializer is keyed on the recorded success status does not recognise the response as the modelled success.
Fix: the TagResource handler's success path in the location service crate (its response serialization) — the empty-output response must be emitted with the service's recorded 204 status rather than the crate's default empty-200.
Traces: location-geofence-tag-resource-lifecycle#task#2.

Cause 2 of 3: UntagResource answers HTTP 200 where the service answers 204.
AWS: Smithy: UntagResource DELETE /tags/{ResourceArn} with TagKeys bound to @httpQuery "tagKeys", @idempotent, output shape UntagResourceResponse with NO members and again no explicit @http code (code 200 reported as codeDefaulted). Recorded behavior only: a successful UntagResource answers HTTP 204 with an empty body.
Emulator: answers HTTP 200 (body empty).
Why: same as cause 1 — the untag call carries no payload, so a wrong status is the whole divergence, and it is the value clients key their success handling on.
Fix: the UntagResource handler's success path in the location service crate (its response serialization), the same empty-output serializer cause 1 points at.
Traces: location-geofence-tag-resource-lifecycle#task#4.

Cause 3 of 3: the ListTagsForResource not-found refusal re-words the message and interpolates a resource name the service does not echo.
AWS: Smithy: ListTagsForResource GET /tags/{ResourceArn} (@readonly), error ResourceNotFoundException (errorKind client, httpError 404) with one required member Message (jsonName "message"), documented as "The resource that you've entered was not found in your AWS account." Recorded behavior only for the wording: listing tags for an ARN that names a geofence collection which does not exist answers 404 with message "Resource was not found" — that is entirely FIXED text; no part of it is taken from the request or from emulator state, and in particular the collection name from the ARN is NOT interpolated into it.
Emulator: answers 404 (status is correct) with message "Geofence collection not found: <CollectionName>." — i.e. the crate's generic per-resource-type template, whose fixed part ("Geofence collection not found: " plus a terminating period) is not the service's text and whose variable part leaks the collection name parsed out of the request ARN into a message AWS keeps constant.
Why: the tagging plane is ARN-generic — one refusal sentence covers every taggable Location resource — so a client that matches the documented refusal text, logs it, or asserts that the message does not disclose the requested resource name sees text the service never emits. The replay additionally reports $headers.x-amzn-errortype diverging in shape from the recorded value; the recorded header value is not asserted by the fixture, so no required form for that header is claimed here, but the emulator's bare "ResourceNotFoundException" does not match the recorded shape and should be emitted in whatever form the recording carries.
Fix: the ListTagsForResource handler's not-found path in the location service crate — raise ResourceNotFoundException with the tagging plane's single fixed sentence, rather than routing through the crate's generic resource-type/name not-found template, and emit x-amzn-errortype in the service's form.
Traces: location-geofence-tag-resource-lifecycle#task#6.

Related open issues (neither covers these steps): "Location refusal bodies are emulator-authored and omit ValidationException Reason and FieldList" (20261029T234705Z) covers the CreateTracker/DescribeTracker/GetDevicePosition/GetMapGlyphs refusal wording, and "Location GetGeofence diverges..." (20261105T212519Z) covers the geofence read path; the tagging plane's statuses and its ARN-generic not-found sentence are not owned by either.

## Evidence
Fixture location-geofence-tag-resource-lifecycle, replayed against the emulator (emulator up; /__admin/health 200):

Cause 1 — step task#2 (TagResource): status expected 204, got 200. Divergence key service=location|TagResource|status=204->200|keys=status. Body empty on both sides, so the status is the only difference.

Cause 2 — step task#4 (UntagResource): status expected 204, got 200. Divergence key service=location|UntagResource|status=204->200|keys=status. Body empty on both sides.

Cause 3 — step task#6 (ListTagsForResource for an ARN naming a missing geofence collection), 404 on both sides:
  $.message: expected 'Resource was not found' (all fixed text), got 'Geofence collection not found: sim2real-probe-location-missing-collection-b34650fd.' — the fixed part 'Geofence collection not found: ' + '.' is the emulator's template, and the collection name is a value lifted from the request ARN which the recorded message does not contain.
  $headers.x-amzn-errortype: shape_mismatch — recorded value '<shaped-not-asserted>', emulator sent 'ResourceNotFoundException'.
  Divergence key service=location|ListTagsForResource|status=404->404|keys=$.message,$headers.x-amzn-errortype.

Model evidence: Smithy TagResource -> output TagResourceResponse {} , http {method: POST, uri: /tags/{ResourceArn}, code: 200, codeDefaulted: true}; UntagResource -> output UntagResourceResponse {}, http {method: DELETE, uri: /tags/{ResourceArn}, code: 200, codeDefaulted: true}, idempotent; ListTagsForResource -> http {method: GET, uri: /tags/{ResourceArn}}, errors include ResourceNotFoundException (client, httpError 404, required Message jsonName 'message').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture location-geofence-tag-resource-lifecycle from corpus simparity-wire-aws-v1.
2. Cause 1 (task#2): create a geofence collection, then call TagResource POST /tags/{collectionArn} with a tag map. Observe HTTP 200 with an empty body; the recording has HTTP 204 with an empty body.
3. Cause 2 (task#4): call UntagResource DELETE /tags/{collectionArn}?tagKeys=<key>. Observe HTTP 200; the recording has HTTP 204.
4. Cause 3 (task#6): call ListTagsForResource GET /tags/{arn} for an ARN whose geofence collection does not exist. Observe 404 with message 'Geofence collection not found: <CollectionName>.' instead of the fixed sentence 'Resource was not found', and x-amzn-errortype emitted as the bare 'ResourceNotFoundException'.
5. All three must pass on a re-replay of location-geofence-tag-resource-lifecycle steps task#2, task#4 and task#6 before the issue is closed.
