Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-tag-resource-list-tags-roundtrip
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-tag-resource-list-tags-roundtrip emr-serverless-untag-resource-removes-tag
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|TagResource|2xx->2xx|status
Issue-Key: emr-serverless|UntagResource|2xx->2xx|status
Issue-Commit: e63ec425c23513ba26a0bb6d325f047894e211cc

# EMR Serverless TagResource and UntagResource answer 200 instead of AWS's 204 No Content

> _Note: this issue was identified against the code base at commit `e63ec425c23513ba26a0bb6d325f047894e211cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Both EMR Serverless tagging mutations return an empty output shape, and the emulator serializes that empty body with HTTP 200. AWS answers these two operations with 204 No Content. The tag/untag side effects themselves are correct (the subsequent ListTagsForResource steps in both fixtures pass), so the defect is purely the success status the two handlers select. 2 causes.

Cause 1 of 2: TagResource success status.
AWS: TagResource is POST /tags/{resourceArn} with an empty output structure (Smithy: TagResource output TagResourceResponse has no members; the @http code is the protocol default 200, i.e. not explicitly stated by the model — codeDefaulted). The recorded AWS response for a successful tagging is HTTP 204 with no body, so for the success status this is recorded behavior only and the recording governs over the defaulted model code. There is no body and therefore no message or error code involved.
Emulator: HTTP 200 (empty body) for the same successful TagResource call.
Why: a client or proxy that keys off the status — asserting 204 for a no-content mutation, or treating a 200 as "a body follows" and attempting to parse it — diverges from AWS; status-code assertions in conformance tests and HTTP-level caches/gateways see a different response class than production.
Fix: the TagResource handler's success response construction in the emr-serverless service crate — return 204 with no body rather than 200.
Traces: emr-serverless-tag-resource-list-tags-roundtrip#task#3.

Cause 2 of 2: UntagResource success status.
AWS: UntagResource is DELETE /tags/{resourceArn} with tagKeys as an @httpQuery list and an empty output structure (Smithy: UntagResource output UntagResourceResponse has no members; @http code defaulted to 200, @idempotent). The recorded AWS response for a successful untagging is HTTP 204 with no body — recorded behavior only for the status, same as above.
Emulator: HTTP 200 (empty body) for the same successful UntagResource call.
Why: identical to cause 1 — the no-content status contract of the tagging mutations is not reproduced, so status-sensitive clients and intermediaries behave differently against the emulator than against AWS.
Fix: the UntagResource handler's success response construction in the emr-serverless service crate, alongside TagResource's.
Traces: emr-serverless-untag-resource-removes-tag#task#3.

## Evidence
Cause 1 — fixture emr-serverless-tag-resource-list-tags-roundtrip, step task#3, TagResource:
  expected: status 204, no body
  actual:   status 200
  replay detail: status: expected 204, got 200 (no other key diverged)

Cause 2 — fixture emr-serverless-untag-resource-removes-tag, step task#3, UntagResource:
  expected: status 204, no body
  actual:   status 200
  replay detail: status: expected 204, got 200 (no other key diverged)

Model grounding: Smithy TagResource http {method POST, uri /tags/{resourceArn}, code 200, codeDefaulted true}, output TagResourceResponse with no members; UntagResource http {method DELETE, uri /tags/{resourceArn}, code 200, codeDefaulted true}, output UntagResourceResponse with no members, @idempotent. Because the model's code is defaulted rather than stated, the recorded 204 is the requirement for both.

## Steps to Reproduce
1. Replay fixture emr-serverless-tag-resource-list-tags-roundtrip: create an application, then call TagResource (POST /tags/{applicationArn}) with a tag map. Observe HTTP 200 where AWS returns 204 No Content. The following ListTagsForResource step passes, confirming the tag was applied.
2. Replay fixture emr-serverless-untag-resource-removes-tag: tag an application, then call UntagResource (DELETE /tags/{applicationArn}?tagKeys=<key>). Observe HTTP 200 where AWS returns 204 No Content.
