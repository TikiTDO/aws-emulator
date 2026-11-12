Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-virtual-cluster-mutation-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-virtual-cluster-mutation-error-paths
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|TagResource|4xx->4xx|$.message
Issue-Key: emr-containers|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|ListTagsForResource|4xx->4xx|$.message
Issue-Key: emr-containers|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-containers|UntagResource|4xx->4xx|$.message
Issue-Key: emr-containers|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# emr-containers tag APIs never validate resourceArn's ARN form, answering ResourceNotFoundException instead of a ValidationException

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new root causes in the emr-containers emulator, all the same defect in the three `/tags/{resourceArn}` operations: the `resourceArn` HTTP label is never checked against the modeled ARN form, so a value that is not an emr-containers virtual-cluster ARN at all is taken as a bare virtual-cluster id and looked up in state. The refusal that comes back is therefore the wrong error class with the wrong message. All three causes must be fixed and all three fixture steps must replay clean before this issue is closed.

Common model evidence: `resourceArn` targets `RsiArn` and is `@required` + `@httpLabel` with `@length min 60 max 500` and `@pattern ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$` on all three operations; each models `InternalServerException` (500), `ResourceNotFoundException` (400) and `ValidationException` (400). A value that violates the length/pattern constraint is a constraint violation, i.e. a `ValidationException`, and only a well-formed ARN whose cluster is absent is a `ResourceNotFoundException`. The emulator collapses the two.

Cause 1 of 3 - TagResource (POST /tags/{resourceArn}).
AWS: Smithy: TagResource.resourceArn = RsiArn, @required @httpLabel, @length 60..500, @pattern ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$; errors ValidationException(400), ResourceNotFoundException(400), InternalServerException(500). Recorded behavior: HTTP 400 with message `Invalid input resource arn` and an x-amzn-errortype that is NOT `ResourceNotFoundException`. That message is entirely FIXED text for this error class - it has no variable parts and in particular does not echo the supplied value.
Emulator: HTTP 400, but `x-amzn-errortype: ResourceNotFoundException` and message `Virtual cluster <value> doesn't exist.` - the shared not-found wording with the supplied label interpolated as if it were a `ResourceIdString`. (Fixed text `Virtual cluster `/` doesn't exist.`, variable part the raw label taken from the request URI.)
Why: a client cannot tell a malformed-ARN programming error from a genuinely absent cluster. Code that retries or reports a missing resource on `ResourceNotFoundException` retries a request that can never succeed, and code that matches AWS's fixed `Invalid input resource arn` sentence never fires. The emulator also accepts, as a resource reference, a label real AWS rejects before any lookup, so a caller that is silently passing a cluster id where an ARN is required appears to work.
Fix: the input validation of TagResource in the emr-containers service crate - enforce the RsiArn @length/@pattern constraint on the `resourceArn` label before the state lookup and raise ValidationException with AWS's fixed sentence; keep ResourceNotFoundException for a well-formed ARN whose virtual cluster is absent.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#3.

Cause 2 of 3 - ListTagsForResource (GET /tags/{resourceArn}).
AWS: Smithy: ListTagsForResource.resourceArn carries the identical RsiArn @length 60..500 / @pattern constraint and the same three modeled errors; output is `tags` (TagMap). Recorded behavior: HTTP 400 with message `Invalid input resource arn` and an x-amzn-errortype other than `ResourceNotFoundException` - same fixed text, same error class as Cause 1.
Emulator: HTTP 400 with `x-amzn-errortype: ResourceNotFoundException` and message `Virtual cluster <value> doesn't exist.`.
Why: same as Cause 1 - a read path that misreports a malformed reference as a missing resource makes a caller believe the ARN it sent was well formed and the cluster gone.
Fix: the input validation of ListTagsForResource in the emr-containers service crate, same constraint check and same fixed ValidationException wording.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#4.

Cause 3 of 3 - UntagResource (DELETE /tags/{resourceArn}).
AWS: Smithy: UntagResource.resourceArn carries the identical RsiArn @length 60..500 / @pattern constraint, `tagKeys` is @required @httpQuery, and the same three errors are modeled. Recorded behavior: HTTP 400 with message `Invalid input resource arn` and an x-amzn-errortype other than `ResourceNotFoundException`.
Emulator: HTTP 400 with `x-amzn-errortype: ResourceNotFoundException` and message `Virtual cluster <value> doesn't exist.`.
Why: same as Cause 1. Teardown code that treats `ResourceNotFoundException` from an untag as "already gone, nothing to do" silently swallows a malformed-ARN bug that AWS would surface as a validation failure.
Fix: the input validation of UntagResource in the emr-containers service crate, same constraint check and same fixed ValidationException wording. All three handlers should share one `resourceArn` validator so the three paths cannot drift.
Traces: fixture emr-containers-virtual-cluster-mutation-error-paths step task#5.

Related but not covering: open issue 20261029T185510Z-emr-containers-validationexception-refusals-carry-the-emulator-s.md is the same family of wording defect but is scoped to job templates, security configurations and client-token conflicts, and does not touch the three /tags/{resourceArn} operations or the missing ARN-form constraint described here. Open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md owns the CreateVirtualCluster/DescribeJobRun steps of this corpus, not these three.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (35 steps, 24 passed, 11 mismatched, score 68.6%). Three of the mismatches are the clusters below; all three are x-amzn-errortype shape mismatches plus a message value mismatch, with the status already correct at 400.

Cause 1 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#3, TagResource, expected 400 / got 400:
  $.message: value_mismatch (expected 'Invalid input resource arn', got "Virtual cluster sim2realprobenonexistentvc00 doesn't exist.")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 2 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#4, ListTagsForResource, expected 400 / got 400:
  $.message: value_mismatch (expected 'Invalid input resource arn', got "Virtual cluster sim2realprobenonexistentvc00 doesn't exist.")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 3 - [mismatch] emr-containers-virtual-cluster-mutation-error-paths#task#5, UntagResource, expected 400 / got 400:
  $.message: value_mismatch (expected 'Invalid input resource arn', got "Virtual cluster sim2realprobenonexistentvc00 doesn't exist.")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

The string `sim2realprobenonexistentvc00` in the actual messages is the probe's per-run identifier, not part of the requirement: the DEFECT is that AWS's message for this class is the fixed sentence 'Invalid input resource arn' with NO variable part, while the emulator emits its not-found template with the request's label interpolated. Likewise the recorded x-amzn-errortype is asserted by shape rather than literally; what the replay establishes is that it is not `ResourceNotFoundException`, which is what the emulator sends.

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query operation=TagResource,UntagResource,ListTagsForResource:
  TagResource: POST /tags/{resourceArn}; resourceArn -> RsiArn, required, httpLabel, length{min:60,max:500}, pattern ^arn:(aws[a-zA-Z0-9-]*):emr-containers:.+:(\d{12}):/virtualclusters/.+$; errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  ListTagsForResource: GET /tags/{resourceArn}; same resourceArn constraints and same three errors.
  UntagResource: DELETE /tags/{resourceArn}; same resourceArn constraints and same three errors.
The value the probe sent is far shorter than the modeled minimum of 60 characters and matches none of the ARN pattern, so under the model it can only be a constraint violation.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay wire corpus simparity-wire-aws-v1, fixture emr-containers-virtual-cluster-mutation-error-paths.
3. Cause 1 (step task#3): call TagResource with a `resourceArn` that is not an emr-containers virtual-cluster ARN (e.g. a bare virtual-cluster id, shorter than the modeled 60-character minimum) and any tags map. Expect HTTP 400 with message exactly 'Invalid input resource arn' and an x-amzn-errortype that is NOT ResourceNotFoundException; observed HTTP 400, x-amzn-errortype ResourceNotFoundException and message "Virtual cluster <value> doesn't exist.".
4. Cause 2 (step task#4): call ListTagsForResource with the same malformed `resourceArn`. Same expectation, same observed divergence.
5. Cause 3 (step task#5): call UntagResource with the same malformed `resourceArn` and any tagKeys. Same expectation, same observed divergence.
6. Confirm the complementary case still behaves: a WELL-FORMED emr-containers virtual-cluster ARN whose cluster does not exist must still answer ResourceNotFoundException - that path must not be changed into a ValidationException.
7. Re-replay fixture emr-containers-virtual-cluster-mutation-error-paths steps task#3, task#4 and task#5; all three must pass.
