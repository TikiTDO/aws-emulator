Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-compliance-inquiry-feedback-and-export
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-compliance-inquiry-feedback-and-export
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|GetComplianceInquiryMetadata|2xx->2xx|$.tags
Issue-Key: artifact|ExportComplianceInquiry|2xx->2xx|$.tags
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# artifact compliance-inquiry responses omit the empty tags map AWS serializes

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComplianceInquiryMetadata

## Description
The artifact emulator's compliance-inquiry read and export paths drop the modeled output member `tags` from their 200 response bodies whenever the inquiry carries no tags, instead of serializing it as a present, empty JSON object. Both operations model `tags` as a `TagsMap` (@length max 50) on their output shape, and the recorded AWS responses carry `"tags": {}` on an untagged inquiry. This issue bundles 2 net-new causes, both on service `artifact`, both the same serialization fault on a different operation; both must be fixed before it is closed.

Cause 1 of 2: GetComplianceInquiryMetadata omits `tags` (fixture artifact-compliance-inquiry-feedback-and-export, step task#3).
AWS: Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json) models GetComplianceInquiryMetadata -> GetComplianceInquiryMetadataResponse with members `complianceInquiryDetail` (InquiryDetail) and `tags` (TagsMap, @length max 50); HTTP GET /v1/compliance-inquiry/getMetadata, success code 200. The recorded AWS response for an inquiry with no tags is status 200 with `tags` PRESENT and equal to the empty object `{}` (recorded behavior for the empty case; the model states the member, the recording states that an empty map is emitted rather than suppressed).
Emulator: answers 200 but the body has no `tags` key at all — replay reports `$.tags: missing (expected {}, got None)`.
Why: a client that reads the tag set directly off the response (SDKs surface it as a map, and generated code for a non-sparse map member distinguishes "absent" from "empty") sees null/absent where AWS guarantees a map it can iterate. Code that iterates `response.tags` without a null check faults, and code that treats absent as "unknown" (rather than "no tags") will re-read or mis-report the inquiry's tagging state.
Fix: the GetComplianceInquiryMetadata response serializer in the artifact service crate — emit `tags` unconditionally from the inquiry's tag state, serializing an empty map as `{}` instead of skipping the member when the map is empty.

Cause 2 of 2: ExportComplianceInquiry omits `tags` (fixture artifact-compliance-inquiry-feedback-and-export, step task#4).
AWS: Smithy models ExportComplianceInquiry -> ExportComplianceInquiryResponse with members `documentPresignedUrl` (PresignedUrl, @sensitive) and `tags` (TagsMap, @length max 50); HTTP POST /v1/compliance-inquiry/export, success code 200. The recorded AWS response for an untagged inquiry is status 200 with `tags` present as `{}`.
Emulator: answers 200 with `documentPresignedUrl` but no `tags` key — replay reports `$.tags: missing (expected {}, got None)`.
Why: same client-visible consequence as cause 1 on the export path: the export response's tag map is absent rather than empty, so a caller that copies or asserts on the exported inquiry's tags gets null instead of an empty map.
Fix: the ExportComplianceInquiry response serializer in the artifact service crate — same change: always include `tags`, empty map when the inquiry has none.

Neither cause is covered by an open issue. The open artifact issues 20261030T084723Z-artifact-list-operations-answer-200-with-the-recorded.md (list reads return 200 with recorded resources absent), 20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where.md (refusals answered 200/401 instead of 400 ValidationException), 20261107T124604Z-artifact-listreports-catalog-enumeration-omits-nearly-every-published.md and 20261106T180151Z-artifact-listreports-second-page-omits-a-report-the-catalog.md are about missing list members and wrong refusal statuses, not about an empty modeled map member being suppressed on a successful compliance-inquiry response; the replay reports both of these clusters with an empty owned_by.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (score 74.2%, 8/31 steps mismatched).

Cause 1 — fixture artifact-compliance-inquiry-feedback-and-export, step task#3, operation GetComplianceInquiryMetadata, verdict mismatch, expected_status 200, actual_status 200:
  detail: `$.tags: missing (expected {}, got None)`
  divergence_key: `service=artifact|GetComplianceInquiryMetadata|status=200->200|keys=$.tags`
  Expected body (AWS): 200 with `complianceInquiryDetail` plus `"tags": {}`.
  Actual body (emulator): 200 with no `tags` member.

Cause 2 — fixture artifact-compliance-inquiry-feedback-and-export, step task#4, operation ExportComplianceInquiry, verdict mismatch, expected_status 200, actual_status 200:
  detail: `$.tags: missing (expected {}, got None)`
  divergence_key: `service=artifact|ExportComplianceInquiry|status=200->200|keys=$.tags`
  Expected body (AWS): 200 with `documentPresignedUrl` plus `"tags": {}`.
  Actual body (emulator): 200 with no `tags` member.

Model evidence (smithy_query, model_path services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json):
  GetComplianceInquiryMetadata output = {complianceInquiryDetail: InquiryDetail, tags: TagsMap @length max 50}
  ExportComplianceInquiry output = {documentPresignedUrl: PresignedUrl @sensitive, tags: TagsMap @length max 50}
No per-run value is involved: the divergence is the presence of the member, not any id, host, timestamp or request id.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay wire fixture `artifact-compliance-inquiry-feedback-and-export` from trace set simparity-wire-aws-v1.
3. Step task#3 issues GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<inquiry created earlier in the fixture> (an inquiry with no tags). Observe the 200 body contains `complianceInquiryDetail` but no `tags` member; AWS returns `"tags": {}`.
4. Step task#4 issues POST /v1/compliance-inquiry/export for the same inquiry. Observe the 200 body contains `documentPresignedUrl` but no `tags` member; AWS returns `"tags": {}`.
5. Both steps are reported as mismatches with `$.tags: missing (expected {}, got None)`. After the fix, re-replay artifact-compliance-inquiry-feedback-and-export#task#3 and artifact-compliance-inquiry-feedback-and-export#task#4; both must pass.
