Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-component-tagging-workflow
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-component-tagging-workflow
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|UntagResource|2xx->2xx|status
Issue-Commit: 7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d

# Image Builder UntagResource answers HTTP 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Cause 1 of 1: UntagResource returns the wrong HTTP success status for its empty-output tag mutation.

AWS: Image Builder's UntagResource is a DELETE on /tags/{resourceArn} (Smithy: UntagResource http {method: DELETE, uri: /tags/{resourceArn}, code: 200}; input resourceArn @required @httpLabel ImageBuilderArn with the imagebuilder ARN pattern, tagKeys @required @httpQuery('tagKeys') TagKeyList @length(min 1, max 50); output shape UntagResourceResponse has NO members; modeled errors InvalidParameterException 400, ResourceNotFoundException 404, ServiceException 500). The real service answers the successful, empty-output tag removal with HTTP 204 No Content and an empty body — recorded behavior only for the status, since the model's declared code (200) disagrees with what the service actually sends and the recorded response is the authority here: it is 204. This is a success-path status only, so no error code and no error message text are involved.

Emulator: the untagging call succeeds and the named keys are removed from the resource's tag map, but the response carries HTTP 200 instead of 204. The status is fixed at 200 by the operation's success-status constant rather than taking the no-content status the service uses for this empty-output mutation.

Why: a client that keys off the response status cannot distinguish the outcomes it is written for. Code that treats 204 as the success signal for tag mutations (a strict status assertion, a conditional accepting only 204 as 'applied, no content', or a generated client layer that selects the no-content body path on 204) sees an unexpected 200 and either reports the tag removal as a failure or attempts to parse a body where AWS sends none. It also leaves the two tag mutations inconsistent with the rest of the no-content mutation surface, so HTTP-level recorders and conformance suites diverge on an operation whose side effect actually worked.

Fix: the operation's success-status binding in its handler/serializer, not the state code — the state effect is already correct (the handler removes exactly the named keys under one guard and normalises an emptied map back to None). The success status the response serializer emits for the empty UntagResourceOutput is what must become 204, and it should be kept in agreement with the sibling TagResource path so both tag mutations answer the same way.

Note: the open issue 20261102T010307Z-imagebuilder-image-builder-tagresource-answers-http-200-instead-of.md records the SAME fault on TagResource (and even suggests checking the sibling untag path), but it carries only the TagResource replay key (fixture imagebuilder-component-tagging-workflow step task#3) and does not cover this one; the replay reported this UntagResource cluster with an empty stub_path and no owning issue.

Traces: imagebuilder-component-tagging-workflow#task#5 (UntagResource, expected 204, got 200).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture imagebuilder-component-tagging-workflow, step task#5, operation UntagResource (service imagebuilder), verdict mismatch:

  detail: "status: expected 204, got 200"
  expected_status: 204
  actual_status: 200
  divergence_key: service=imagebuilder|UntagResource|status=204->200|keys=status
  affected: ["imagebuilder-component-tagging-workflow#task#5"] (affected_count 1)
  stub_path: "" (not blocked by an unimplemented operation)
  owned_by: [] (no open issue carries this replay key)

Expected: HTTP 204 with an empty body.
Actual:   HTTP 200 with an empty JSON body.

No body keys diverged and no error was returned — the tag keys were removed; only the success status differs. Source of the fixed status: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/untag_resource.rs line 136, `const SUCCESS_STATUS: u16 = 200u16;`. The handler's success return is `Ok(UntagResourceOutput {})` (line 183), the empty output structure AWS answers with 204.

## Steps to Reproduce
1. Boot the imagebuilder emulator from this checkout.
2. Create a component (CreateComponent) to get a taggable component build-version ARN, and tag it (TagResource) with at least one key/value pair.
3. Call UntagResource with resourceArn set to that ARN and tagKeys naming the key just written.
4. Observe the HTTP response status is 200 with an empty JSON body, where AWS Image Builder answers 204 No Content with an empty body.
5. Equivalently, replay fixture imagebuilder-component-tagging-workflow and observe step task#5 fail with "status: expected 204, got 200" while the tag removal itself succeeds (a following ListTagsForResource/GetComponent no longer shows the key).
