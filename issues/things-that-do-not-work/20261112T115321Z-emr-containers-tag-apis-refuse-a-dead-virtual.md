Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-virtual-cluster-mutation-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-virtual-cluster-mutation-error-paths
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 4839448a699fa31c23507e6b5ab2cdd919186720

# emr-containers tag APIs refuse a dead virtual-cluster ARN with the wrong x-amzn-errortype

> _Note: this issue was identified against the code base at commit `4839448a699fa31c23507e6b5ab2cdd919186720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new root causes in the emr-containers emulator, all the same fault on the three tag APIs bound to /tags/{resourceArn}: when the resourceArn satisfies the modeled ARN constraint but names no existing virtual cluster, the emulator gets the HTTP status right (400) and the `message` right (no message divergence is reported for any of the three steps), but serializes the WRONG error code into the `x-amzn-errortype` header - it classifies the refusal as ValidationException where the recorded AWS response carries a different code. All three causes must be fixed and all three fixture steps must replay clean before this issue is closed.

Common note for all three causes. AWS: Smithy (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json) binds all three operations to /tags/{resourceArn} with `resourceArn` = RsiArn, @required @httpLabel, @length 60..500, @pattern ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$, and models EXACTLY three errors on each: InternalServerException (server, 500), ResourceNotFoundException (client, 400, doc "The specified resource was not found.") and ValidationException (client, 400, doc "There are invalid parameters in the client request."). A syntactically valid ARN that resolves to nothing is a missing resource, not an invalid parameter, so of the two modeled 400 classes the not-found one is the applicable class; the fixture asserts the recorded header by SHAPE rather than by literal, so the exact code must be read back off that step's recorded response rather than guessed (Hypothesis: it is ResourceNotFoundException, the only other 400 the operations model). Requirement in general terms: the error code (and the `__type`/errortype the body and header agree on) for a tag-path refusal on a nonexistent virtual cluster must be the code AWS records for that path, and the message - whose fixed wording the emulator already reproduces, with the resource identifier taken from the request's resourceArn as its variable part - must stay as it is.
Fix (shared): the refusal branch of the tag handlers in the emr-containers service crate, plus the error serializer that writes x-amzn-errortype - classify the ARN-resolves-to-nothing refusal under the recorded not-found error class instead of ValidationException on all three paths. The 400 status and the message text are already correct and must not change. This is a classification change only; do not touch the ValidationException path used for an ARN that actually violates RsiArn's length/pattern constraint, which remains a ValidationException.

Cause 1 of 3 - TagResource on a nonexistent virtual cluster ARN.
AWS: Smithy: TagResource, POST /tags/{resourceArn}, http code 200; input resourceArn (RsiArn, required httpLabel, length 60..500, pattern as above) and tags (TagMap, required, length 0..50); errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400). Recorded behavior: HTTP 400 whose message the emulator already matches and whose x-amzn-errortype is asserted by shape and is NOT ValidationException.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException`.
Why: SDKs dispatch on x-amzn-errortype, not on the message, so a caller that catches the not-found error around a tagging call never catches the emulator's refusal - it surfaces as the wrong modeled exception, and retry/ignore-if-absent logic keyed on the not-found class does not fire; conversely a caller that catches ValidationException catches a condition AWS would not raise here.
Fix: the not-found branch of TagResource's handler/serializer in the emr-containers service crate.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#3.

Cause 2 of 3 - ListTagsForResource on a nonexistent virtual cluster ARN.
AWS: Smithy: ListTagsForResource, GET /tags/{resourceArn}, http code 200; input is resourceArn only (RsiArn, required httpLabel, length 60..500, same pattern); output tags (TagMap, 0..50); errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400). Recorded behavior: HTTP 400, message matching what the emulator already emits, x-amzn-errortype asserted by shape and NOT ValidationException.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException`.
Why: a read path is where callers most often branch on "resource absent" versus "request malformed"; with the wrong code that distinction is unavailable, and a client that treats the not-found class as an empty-tags condition instead raises a validation failure.
Fix: the not-found branch of ListTagsForResource's handler/serializer in the emr-containers service crate.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#4.

Cause 3 of 3 - UntagResource on a nonexistent virtual cluster ARN.
AWS: Smithy: UntagResource, DELETE /tags/{resourceArn}, http code 200; input resourceArn (RsiArn, required httpLabel, length 60..500, same pattern) and tagKeys (TagKeyList, required, @httpQuery "tagKeys", length 0..50); errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400). Recorded behavior: HTTP 400, message matching, x-amzn-errortype asserted by shape and NOT ValidationException.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException`.
Why: teardown code untags best-effort and swallows only the not-found class; against the emulator the refusal arrives as a validation failure and aborts cleanup.
Fix: the not-found branch of UntagResource's handler/serializer in the emr-containers service crate.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#5.

Related but not covering: open issue 20261030T171255Z-emr-containers-delete-and-pagination-refusals-diverge-in.md is the same error-code family and even hypothesizes about these tag paths, but its four causes are scoped to DeleteVirtualCluster, DeleteManagedEndpoint, ListJobTemplates and CancelJobRun and none of its Issue-Keys or trace ids covers task#3, task#4 or task#5 of this fixture - the replay reports these three clusters as unowned. Open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md owns the CreateVirtualCluster and DescribeJobRun steps of this corpus, not these.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout: 35 steps, 26 passed, 9 mismatched, 0 errored, 0 blocked, score 74.3%, emulator_up true (/__admin/health answered 200). Six of the nine mismatches are already owned by open issues; the three below are unowned and have empty stub_path.

Cause 1 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#3, TagResource, expected 400 / got 400:
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  divergence_key: service=emr-containers|TagResource|status=400->400|keys=$headers.x-amzn-errortype
  no $.message divergence was reported, so the message is already correct and only the error code is wrong.

Cause 2 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#4, ListTagsForResource, expected 400 / got 400:
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  divergence_key: service=emr-containers|ListTagsForResource|status=400->400|keys=$headers.x-amzn-errortype

Cause 3 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#5, UntagResource, expected 400 / got 400:
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  divergence_key: service=emr-containers|UntagResource|status=400->400|keys=$headers.x-amzn-errortype

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query query=operation operation=TagResource,UntagResource,ListTagsForResource and query=shape shape=ResourceNotFoundException,ValidationException:
  TagResource: POST /tags/{resourceArn}, code 200; resourceArn -> RsiArn required httpLabel length{60,500} pattern ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$; tags -> TagMap required length{0,50}; errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  UntagResource: DELETE /tags/{resourceArn}, code 200; tagKeys -> TagKeyList required httpQuery 'tagKeys' length{0,50}; same errors.
  ListTagsForResource: GET /tags/{resourceArn}, code 200; output tags -> TagMap length{0,50}; same errors.
  ResourceNotFoundException: client error, httpError 400, member message -> String1024 length{1,1024} pattern \S, doc 'The specified resource was not found.'
  ValidationException: client error, httpError 400, same message member, doc 'There are invalid parameters in the client request.'
So both candidate 400 classes exist on all three operations, and the emulator picks the invalid-parameters one for a refusal that is a missing resource; the recorded code is asserted by shape and differs from ValidationException.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay wire corpus simparity-wire-aws-v1, fixture emr-containers-virtual-cluster-mutation-error-paths.
3. Cause 1 (step task#3): call TagResource with a resourceArn that satisfies RsiArn (60..500 chars, matching ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$) but names no existing virtual cluster, and a non-empty tags map. Expect HTTP 400 carrying the error code recorded in that step's x-amzn-errortype (NOT ValidationException; per the model the applicable modeled class is ResourceNotFoundException) with the message the emulator already produces; observed x-amzn-errortype ValidationException.
4. Cause 2 (step task#4): call ListTagsForResource with the same kind of dead-but-valid resourceArn. Same expectation; observed x-amzn-errortype ValidationException.
5. Cause 3 (step task#5): call UntagResource with the same kind of dead-but-valid resourceArn and a tagKeys query parameter. Same expectation; observed x-amzn-errortype ValidationException.
6. Check the negative case is unchanged: a resourceArn that actually violates RsiArn's length or pattern must still be refused as ValidationException with its existing message.
7. Re-replay emr-containers-virtual-cluster-mutation-error-paths steps task#3, task#4 and task#5; all three must pass.
