Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-not-found-resources
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-not-found-resources
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|UntagResource|4xx->4xx|$.message
Issue-Key: pca-connector-scep|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-scep|UntagResource|4xx->4xx|status
Issue-Commit: fd57799079cf971de3edcb569520acb45a060d60

# UntagResource on an absent resource answers 404 ResourceNotFoundException instead of the tagging layer's 400 NotFoundException

> _Note: this issue was identified against the code base at commit `fd57799079cf971de3edcb569520acb45a060d60`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Single cause, in pca-connector-scep. 1 cause.

Cause 1 of 1: UntagResource against a resource ARN that names nothing is refused with the service's own 404 ResourceNotFoundException, where AWS answers the tagging subsystem's 400 NotFoundException.

  AWS: HTTP 400 with response header x-amzn-errortype = `NotFoundException` and a JSON body whose lowercase member `message` is the fixed string "Resource was not found" — the whole message is fixed text, nothing is interpolated from the request (no ARN, no resource type), so there is no variable part to render. Smithy (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json): UntagResource is DELETE /tags/{ResourceArn} with output Unit and success code 204; UntagResourceRequest.ResourceArn is a required @httpLabel targeting plain String (no @pattern), TagKeys a required @httpQuery "tagKeys"; its modeled errors are AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). The model therefore does NOT list the error AWS actually returns here: that the three tagging endpoints answer absence from the tag subsystem with a 400 `NotFoundException` carrying a lowercase `message` is recorded behavior only, taken from the recorded response of this fixture step. It is the same shape AWS returns on the sibling tag paths.

  Emulator: HTTP 404 with x-amzn-errortype = `ResourceNotFoundException`; the expected lowercase `message` member is absent entirely (the emulator emits the service's capital-M `Message`/`ResourceId`/`ResourceType` not-found envelope instead).

  Why: a client gets both the wrong status class and the wrong error code on the untag path. A catch/branch on NotFoundException never fires; a retry/error-classification policy that distinguishes a generic 4xx tagging refusal from a 404 resource miss classifies the call differently; and because the body member name differs in case (`message` vs `Message`), even reading the message text off the error fails. Cleanup code that untags a resource which was concurrently deleted sees an error class AWS never emits on this operation.

  Fix: the UntagResource handler's not-found branch — i.e. the shared tag-resource lookup that the tagging endpoints reach — must emit the 400 NotFoundException shape with a lowercase `message` rather than constructing the service's 404 ResourceNotFoundException. This is the same branch the ListTagsForResource and TagResource paths need.

  Traces: pca-connector-scep-error-not-found-resources#task#8.

Related but NOT covering this cause: open issue 20261029T190804Z-pca-connector-scep-missing-resource-handling-returns-the-wrong-status-class.md describes the identical divergence for ListTagsForResource (its cause 3) and TagResource (its cause 4), but carries no replay key for the UntagResource step — its Issue-Key/Issue-Trace-Ids list covers only ListTagsForResource, TagResource, DeleteConnector, DeleteChallenge and CreateConnector, so a replay of that issue never exercises UntagResource. Open issue 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md is behavioral and mentions a separate untag-resource-removes-tag check; it carries no wire replay key for this step either.

## Evidence
Replay against trace set simparity-wire-aws-v1, 1 net-new mismatch cluster (stub_path empty, owned_by empty):

- pca-connector-scep-error-not-found-resources#task#8 UntagResource: status expected 400, got 404; $.message expected 'Resource was not found', got None (member absent); $headers.x-amzn-errortype expected 'NotFoundException', got 'ResourceNotFoundException'.
  divergence_key: service=pca-connector-scep|UntagResource|status=400->404|keys=$.message,$headers.x-amzn-errortype,status

Requirements: status 400, x-amzn-errortype `NotFoundException`, body member `message` (lowercase) with the fixed text "Resource was not found" and no interpolated values. The resource ARN/UUID appearing in the emulator's current 404 body is a per-run value and is not part of any requirement — AWS's message for this error class carries no ARN at all.

The remaining 15 divergence clusters in this replay are all pca-connector-scep and all carry an owned_by open issue, so they are not re-filed here.

## Steps to Reproduce
1. Boot the emulator and replay fixture pca-connector-scep-error-not-found-resources from trace set simparity-wire-aws-v1.
2. Step task#8 sends DELETE /tags/{ResourceArn}?tagKeys=... for a connector ARN that was never created.
3. Expect HTTP 400 with header x-amzn-errortype: NotFoundException and body {"message":"Resource was not found"}.
4. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException and the service's capital-M Message/ResourceId/ResourceType not-found envelope; no lowercase `message` member is present.
5. Compare with the sibling paths ListTagsForResource (task#6) and TagResource (task#7) in the same fixture, which diverge identically.
