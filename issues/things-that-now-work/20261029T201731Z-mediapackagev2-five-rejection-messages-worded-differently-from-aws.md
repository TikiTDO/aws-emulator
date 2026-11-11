Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2 mediapackagev2.create-harvest-job-end-time-before-start-time-rejected mediapackagev2.harvest-job-end-time-before-start-time-rejected mediapackagev2.create-origin-endpoint-container-manifest-mismatch-rejected mediapackagev2.origin-endpoint-container-manifest-mismatch-rejected mediapackagev2.put-channel-policy-invalid-json-rejected mediapackagev2.put-origin-endpoint-policy-invalid-policy-rejected mediapackagev2.tag-resource-nonexistent-resource-arn-rejected mediapackagev2.cmaf-ingest-native-protocol-round-trip mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found|run#15|AssertionError|assert err.response["Error"]["Message"] == expected_message, err.response
Issue-Key: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2|run#19|AssertionError|assert err["Message"] == expected_message, f"got {err!r}"
Issue-Key: mediapackagev2.create-harvest-job-end-time-before-start-time-rejected|run#10|AssertionError|assert err.response["Error"]["Message"] == expected_message, err.response
Issue-Key: mediapackagev2.harvest-job-end-time-before-start-time-rejected|run#22|AssertionError|assert message == END_BEFORE_START, f"expected message {END_BEFORE_START!r}, got {message!r}"
Issue-Key: mediapackagev2.create-origin-endpoint-container-manifest-mismatch-rejected|run#12|AssertionError|assert err["Error"].get("Message") == (
Issue-Key: mediapackagev2.origin-endpoint-container-manifest-mismatch-rejected|run#13|AssertionError|assert message == TS_WITH_DASH, f"expected message {TS_WITH_DASH!r}, got {message!r}"
Issue-Key: mediapackagev2.put-channel-policy-invalid-json-rejected|run#14|AssertionError|assert message == INVALID_POLICY, f"expected message {INVALID_POLICY!r}, got {message!r}"
Issue-Key: mediapackagev2.put-origin-endpoint-policy-invalid-policy-rejected|run#17|AssertionError|assert message == INVALID_POLICY, f"expected message {INVALID_POLICY!r}, got {message!r}"
Issue-Key: mediapackagev2.tag-resource-nonexistent-resource-arn-rejected|run#14|AssertionError|assert message == RESOURCE_NOT_FOUND, (
Issue-Key: mediapackagev2.cmaf-ingest-native-protocol-round-trip|run#16|AssertionError|assert no_auth_status == 403, (
Issue-Key: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable|run#14|AssertionError|assert denied_status == 403, (
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# mediapackagev2: five rejection messages worded differently from AWS, plus ingest/manifest data-plane endpoints answering 404 instead of 403

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelHarvestJob

## Description
Behavioral replay of the mediapackagev2 suite (55/66 checks passed) surfaced SEVEN distinct emulator defects. They are bundled because they all belong to the mediapackagev2 service; a fixer MUST close ALL SEVEN causes below before submitting, and re-replay every check id listed per cause.

In every one of causes 1-5 the emulator already returns the correct HTTP status, the correct error code and (where modeled) the correct discriminator member (ValidationExceptionType / ResourceTypeNotFound); only the human-readable Message text diverges. The Message of an AWS error is part of its contract for that error class: the fixed sentence text is the requirement, and its variable parts are taken from the request/state. In all five cases below AWS's message for the class is entirely FIXED text (it interpolates no request value), so the emulator's paraphrase is wrong in whole. Note that the open wire issue 20261022T103649Z-mediapackagev2-emulator-diverges-from-recorded-aws-responses-missing.md explicitly declined to file the harvest-job not-found Message wording on the grounds that no rule was modeled; the behavioral checks below pin that wording directly, so cause 1 is now grounded and is NOT covered by that issue. None of the other open mediapackagev2 issues cover any cause here.

Cause 1 of 7: ResourceNotFoundException for an unknown HarvestJobName is worded wrongly.
  AWS: CancelHarvestJob models ResourceNotFoundException (Smithy: CancelHarvestJob errors -> ResourceNotFoundException @httpError 404; shape ResourceNotFoundException members Message, ResourceTypeNotFound). For a HarvestJobName that was never created under a real ChannelGroup/Channel/OriginEndpoint chain, the message is the fixed sentence "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again" — all fixed text, no value taken from the request.
  Emulator: same 404 / ResourceNotFoundException / ResourceTypeNotFound=HARVEST_JOB, but Message is "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job name or add a harvest job and then try again" ("harvest job name" for "harvest job id", "add" for "create").
  Why: clients and operators that match or display the service's refusal text (log scrapers, support runbooks, tests pinned to AWS wording) see a message AWS never emits for this class.
  Fix: the CancelHarvestJob handler's not-found path / the shared mediapackagev2 error-message catalog the harvest-job lookup uses.
  Traces: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found, mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2.

Cause 2 of 7: the END_TIME_EARLIER_THAN_START_TIME ValidationException is worded wrongly and blames the wrong member path.
  AWS: CreateHarvestJob models ValidationException (Smithy: CreateHarvestJob errors -> ValidationException @httpError 400; ValidationException members Message + ValidationExceptionType; enum value END_TIME_EARLIER_THAN_START_TIME). When ScheduleConfiguration.EndTime is not later than StartTime the message is the fixed sentence "Validation error encountered at request.scheduleConfiguration: Harvest job end time must be later than the start time. Check your configured start and end times and then try again" — note the member path AWS cites is the containing member request.scheduleConfiguration, and the text carries no request values.
  Emulator: correct 400 / ValidationException / ValidationExceptionType=END_TIME_EARLIER_THAN_START_TIME, but Message is "Validation error encountered at request.scheduleConfiguration.endTime: The end time of a harvest job's schedule must be later than its start time. Correct the schedule and then try again" — a different sentence AND a deeper member path (.endTime) than AWS cites.
  Why: a client that parses the "Validation error encountered at <member>" prefix to map the failure back to a request field is pointed at request.scheduleConfiguration.endTime where AWS points at request.scheduleConfiguration, and the remediation sentence differs.
  Fix: CreateHarvestJob's schedule validation, where the END_TIME_EARLIER_THAN_START_TIME ValidationException is constructed.
  Traces: mediapackagev2.create-harvest-job-end-time-before-start-time-rejected, mediapackagev2.harvest-job-end-time-before-start-time-rejected.

Cause 3 of 7: the TS_CONTAINER_TYPE_WITH_DASH_MANIFEST ValidationException is worded wrongly and blames the wrong member path.
  AWS: CreateOriginEndpoint models ValidationException (Smithy: CreateOriginEndpoint errors -> ValidationException @httpError 400; enum ValidationExceptionType value TS_CONTAINER_TYPE_WITH_DASH_MANIFEST). For ContainerType=TS combined with DashManifests the message is the fixed text "Validation error encountered at request: DASH manifest is not supported on endpoint with TS container type. " — the member path is the bare "request", and AWS terminates this message with a trailing space; no request value appears in it.
  Emulator: correct 400 / ValidationException / ValidationExceptionType=TS_CONTAINER_TYPE_WITH_DASH_MANIFEST, but Message is "Validation error encountered at request.dashManifests: A TS container type can't be combined with a DASH manifest. Remove the manifest, or create an origin endpoint whose container type serves it" (different sentence, member path request.dashManifests, no trailing space).
  Why: same prefix-parsing and wording breakage as cause 2, on the container/manifest compatibility class.
  Fix: CreateOriginEndpoint's container/manifest compatibility validation, where this ValidationException is constructed.
  Traces: mediapackagev2.create-origin-endpoint-container-manifest-mismatch-rejected, mediapackagev2.origin-endpoint-container-manifest-mismatch-rejected.

Cause 4 of 7: the INVALID_POLICY ValidationException on both policy writers is worded wrongly.
  AWS: PutChannelPolicy and PutOriginEndpointPolicy take Policy -> PolicyText (Smithy: required, @length max 6144) and model ValidationException @httpError 400 with enum value INVALID_POLICY. For a Policy body that is not a valid IAM policy document the message is the fixed text "Validation error encountered at request.policy: The policy you entered isn't valid. The policy must be a valid IAM policy. Check your policy and then try again" — all fixed, nothing interpolated from the submitted document.
  Emulator: correct 400 / ValidationException on both operations, but Message is "Validation error encountered at request.policy: The policy you provided isn't a valid policy document. Provide a JSON policy document with a Statement and then try again" — a paraphrase that also leaks an emulator-internal rule ("with a Statement") AWS does not state in this message.
  Why: clients that surface or match the policy-rejection text get wording AWS never sends, and the emulator's text implies a narrower rule (a Statement key) than AWS's "must be a valid IAM policy".
  Fix: the shared policy-document validation used by the PutChannelPolicy and PutOriginEndpointPolicy handlers.
  Traces: mediapackagev2.put-channel-policy-invalid-json-rejected, mediapackagev2.put-origin-endpoint-policy-invalid-policy-rejected.

Cause 5 of 7: the tagging NotFoundException message is worded wrongly.
  AWS: recorded behavior only — the tagging operations are modeled with only ValidationException in their error list (Smithy: TagResource errors -> ValidationException @httpError 400; http POST /tags/{ResourceArn} code 204), yet a ResourceArn that names no existing mediapackagev2 resource is refused with code NotFoundException and the message "Resource was not found" — that entire short sentence is the fixed text; AWS does NOT echo the offending ARN into it.
  Emulator: returns the right code (NotFoundException) for both TagResource and ListTagsForResource, but Message is "The resource arn you provided does not name a resource that exists".
  Why: a client matching on AWS's terse "Resource was not found" never matches, and the emulator's longer sentence sets the expectation that this message describes the ARN when AWS's does not.
  Fix: the tagging handlers' ARN-resolution failure path (shared by TagResource/UntagResource/ListTagsForResource) where the NotFoundException is constructed.
  Traces: mediapackagev2.tag-resource-nonexistent-resource-arn-rejected.

Cause 6 of 7: the channel's CMAF ingest endpoint is not served at all — a push to it is answered by the generic router with 404 UnknownService instead of a 403 denial.
  AWS: recorded behavior only (the ingest data plane is not in the Smithy model; CreateChannel.output.IngestEndpoints[].Url is documented as "The ingest domain URL where the source stream should be sent"). A PUT to that URL with NO Authorization header is refused with HTTP 403 and an "Access denied" body; a PUT signed with credentials that are not authorized for the channel is refused with HTTP 403 and a signature-verification-specific body that is DISTINCT from the no-auth body (the two negative classes are distinguishable, not one hardcoded denial).
  Emulator: the PUT is answered HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — i.e. the ingest hostname the emulator itself minted in IngestEndpoints[].Url is not routed to any mediapackagev2 data-plane handler, so no authentication decision is ever made. (Host names and labels are per-environment and are not the requirement; the status, the absence of any ingest route and the missing 403 denial are.)
  Why: a client exercising the documented live-ingest contract cannot reach the endpoint the control plane advertised, and any code path keyed on a 403 denial (credential rotation, retry-on-AccessDenied, signature debugging) is unreachable; a 404 UnknownService also reads as "wrong service/endpoint" rather than "not authorized".
  Fix: the data-plane request router / host dispatch, which must map a channel's ingest hostname to a mediapackagev2 ingest handler, plus that handler's SigV4 authentication path so an unsigned push yields 403 Access denied and a wrongly-signed push yields a distinct 403 signature denial.
  Traces: mediapackagev2.cmaf-ingest-native-protocol-round-trip.

Cause 7 of 7: an origin endpoint's manifest URL answers 404 before any read policy is granted, where AWS answers 403.
  AWS: recorded behavior only (the egress data plane is not modeled). Unlike MediaPackage v1, a v2 OriginEndpoint has NO anonymous-read grant until a resource policy explicitly allows mediapackagev2:GetObject: a GET of HlsManifests[].Url before PutOriginEndpointPolicy is refused with HTTP 403. Only AFTER a policy granting mediapackagev2:GetObject is attached does the same URL answer 404 (policy allows the read, no content has been ingested).
  Emulator: the pre-policy GET of the manifest URL returns HTTP 404 — the same status as the post-policy no-content case — so the endpoint's egress authorization state is never represented.
  Why: the two states AWS distinguishes (not authorized vs authorized-but-empty) collapse into one. A client that waits for 403 to clear as proof its resource policy took effect never observes the transition, and code that treats 404 as "endpoint exists and is readable, just empty" wrongly concludes an unauthorized endpoint is publicly readable.
  Fix: the egress/manifest data-plane handler for origin endpoints — consult the endpoint's attached resource policy and refuse with 403 when no statement grants mediapackagev2:GetObject, reserving 404 for an authorized read with no content.
  Traces: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable.

## Evidence
Behavioral suite: simparity-behavioral-aws-v1, mediapackagev2 55/66 (83%), 11 mismatched, 0 errored, 0 skipped, bootstrap_signal=false. No failing check was marked ALREADY FILED or OWNED by an open issue.

Cause 1 — mediapackagev2.cancel-harvest-job-unknown-job-name-not-found (and -v2), CancelHarvestJob on a never-created HarvestJobName:
  expected Error.Message: "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again"
  actual   Error.Message: "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job name or add a harvest job and then try again"
  matching on both sides: HTTPStatusCode 404, Error.Code ResourceNotFoundException, x-amzn-errortype ResourceNotFoundException, ResourceTypeNotFound HARVEST_JOB.

Cause 2 — mediapackagev2.create-harvest-job-end-time-before-start-time-rejected and mediapackagev2.harvest-job-end-time-before-start-time-rejected, CreateHarvestJob with EndTime <= StartTime:
  expected Error.Message: "Validation error encountered at request.scheduleConfiguration: Harvest job end time must be later than the start time. Check your configured start and end times and then try again"
  actual   Error.Message: "Validation error encountered at request.scheduleConfiguration.endTime: The end time of a harvest job's schedule must be later than its start time. Correct the schedule and then try again"
  matching: HTTPStatusCode 400, Error.Code ValidationException, ValidationExceptionType END_TIME_EARLIER_THAN_START_TIME.

Cause 3 — mediapackagev2.create-origin-endpoint-container-manifest-mismatch-rejected and mediapackagev2.origin-endpoint-container-manifest-mismatch-rejected, CreateOriginEndpoint ContainerType=TS + DashManifests=[{ManifestName:...}]:
  expected Error.Message: "Validation error encountered at request: DASH manifest is not supported on endpoint with TS container type. " (trailing space is AWS's)
  actual   Error.Message: "Validation error encountered at request.dashManifests: A TS container type can't be combined with a DASH manifest. Remove the manifest, or create an origin endpoint whose container type serves it"
  matching: HTTPStatusCode 400, Error.Code ValidationException, ValidationExceptionType TS_CONTAINER_TYPE_WITH_DASH_MANIFEST.

Cause 4 — mediapackagev2.put-channel-policy-invalid-json-rejected (PutChannelPolicy) and mediapackagev2.put-origin-endpoint-policy-invalid-policy-rejected (PutOriginEndpointPolicy), malformed Policy:
  expected Error.Message: "Validation error encountered at request.policy: The policy you entered isn't valid. The policy must be a valid IAM policy. Check your policy and then try again"
  actual   Error.Message: "Validation error encountered at request.policy: The policy you provided isn't a valid policy document. Provide a JSON policy document with a Statement and then try again"
  matching: HTTPStatusCode 400, Error.Code ValidationException; both checks observe the same wrong text, i.e. one shared validation path.

Cause 5 — mediapackagev2.tag-resource-nonexistent-resource-arn-rejected, TagResource with a fabricated ResourceArn:
  expected Error.Message: "Resource was not found"
  actual   Error.Message: "The resource arn you provided does not name a resource that exists"
  matching: Error.Code NotFoundException (the check's code assertion passed; only the message assertion failed).

Cause 6 — mediapackagev2.cmaf-ingest-native-protocol-round-trip, unauthenticated PUT to CreateChannel's IngestEndpoints[0].Url:
  expected: HTTP 403 with a body containing "access denied"; a wrongly-signed PUT then expected a distinct 403 whose body mentions "signature".
  actual:   HTTP 404, body b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' — the request never reached a mediapackagev2 handler. (The check fails at the first assertion, so the wrongly-signed class was not reached and is unverified; both follow from the missing route.)

Cause 7 — mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable, GET of CreateOriginEndpoint's HlsManifests[0].Url before any PutOriginEndpointPolicy:
  expected: HTTP 403 (no read policy granted yet)
  actual:   HTTP 404 (the manifest host resolved and the emulator answered, so this is the emulator's response, not a reachability failure). Recorded example URL shape: https://<label>.egress.<group-label>.mediapackagev2.<region>.amazonaws.com/out/v1/<group>/<channel>/<endpoint>/index.m3u8 — the labels are per-environment and not the requirement.

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 behavioral checks (suite simparity-behavioral-aws-v1), or reproduce each cause directly with botocore against the emulator endpoint:

Cause 1: CreateChannelGroup -> CreateChannel -> CreateOriginEndpoint -> CreateHarvestJob (schedule entirely in the past), then CancelHarvestJob with a HarvestJobName that was never created under that same real parent chain. Inspect Error.Message of the 404. Checks: mediapackagev2.cancel-harvest-job-unknown-job-name-not-found, mediapackagev2.cancel-harvest-job-unknown-job-name-not-found-v2.

Cause 2: CreateHarvestJob with ScheduleConfiguration={StartTime: t, EndTime: t - 5min}. Inspect Error.Message of the 400 (ValidationExceptionType is already END_TIME_EARLIER_THAN_START_TIME). Checks: mediapackagev2.create-harvest-job-end-time-before-start-time-rejected, mediapackagev2.harvest-job-end-time-before-start-time-rejected.

Cause 3: CreateOriginEndpoint with ContainerType='TS' and DashManifests=[{'ManifestName':'dash1'}]. Inspect Error.Message of the 400. Checks: mediapackagev2.create-origin-endpoint-container-manifest-mismatch-rejected, mediapackagev2.origin-endpoint-container-manifest-mismatch-rejected.

Cause 4: PutChannelPolicy with a Policy string that is not a valid IAM policy document, and PutOriginEndpointPolicy likewise. Inspect Error.Message of each 400. Checks: mediapackagev2.put-channel-policy-invalid-json-rejected, mediapackagev2.put-origin-endpoint-policy-invalid-policy-rejected.

Cause 5: TagResource (POST /tags/{ResourceArn}) with a syntactically valid mediapackagev2 ARN naming no existing resource; also ListTagsForResource on the same ARN. Inspect Error.Message. Check: mediapackagev2.tag-resource-nonexistent-resource-arn-rejected.

Cause 6: CreateChannel with InputType CMAF, take IngestEndpoints[0].Url, and HTTP PUT a body to it with no Authorization header: expect 403 + an "Access denied" body, observe 404 + UnknownService. Repeat with a SigV4-signed PUT and expect a distinct 403 mentioning the signature. Check: mediapackagev2.cmaf-ingest-native-protocol-round-trip.

Cause 7: CreateOriginEndpoint with an HLS manifest, take HlsManifests[0].Url and GET it BEFORE calling PutOriginEndpointPolicy: expect 403, observe 404. Then PutOriginEndpointPolicy granting mediapackagev2:GetObject and GET again: 404 is correct only in this second phase. Check: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable.
