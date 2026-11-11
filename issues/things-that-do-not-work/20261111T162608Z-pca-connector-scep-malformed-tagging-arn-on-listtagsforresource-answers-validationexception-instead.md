Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-scep-error-validation-params
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-scep-error-validation-params
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep|ListTagsForResource|4xx->4xx|$.message
Issue-Key: pca-connector-scep|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: d179e3e0167c8c29ef902d1c4425a28783acb150

# Malformed tagging ARN on ListTagsForResource answers ValidationException instead of 400 BadRequestException "Invalid input resource arn: bad syntax"

> _Note: this issue was identified against the code base at commit `d179e3e0167c8c29ef902d1c4425a28783acb150`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Single cause, in pca-connector-scep. 1 cause.

Cause 1 of 1: GET /tags/{ResourceArn} with a string that is not a syntactically valid ARN is refused by the emulator's generic pattern-constraint validator as a ValidationException, where AWS's tagging layer rejects it with its own BadRequestException and a fixed message.

  AWS: HTTP 400 with response header x-amzn-errortype = `BadRequestException` and a JSON body carrying the member `message` (lowercase, as recorded on the wire) whose value is the fixed string "Invalid input resource arn: bad syntax". The entire message is fixed text: nothing is interpolated from the request, so the offending ARN string, the member name and any regex do NOT appear in it. Smithy (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json): ListTagsForResource is GET /tags/{ResourceArn}, success code 200, output member Tags; ListTagsForResourceRequest.ResourceArn is a required @httpLabel targeting plain `String` with NO @pattern and no length constraint, so there is no modeled constraint to report on this member at all. The operation's modeled errors are AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); BadRequestException is modeled in the service (errorKind=client, httpError=400, required member Message) but is not bound to this operation, so that the tagging path answers an ARN-syntax failure with BadRequestException — and serializes its message member in lowercase — is recorded behavior only, taken from the recorded response of this fixture step.

  Emulator: HTTP 400 (status class is right) with x-amzn-errortype = `ValidationException`, and no lowercase `message` member carrying AWS's text at all — the replay reports `$.message: missing (expected 'Invalid input resource arn: bad syntax', got None)`. The emulator routes the unconstrained path member through its shared pattern-constraint message builder and emits a synthesized constraint error for a member that has no modeled pattern.

  Why: a client that catches BadRequestException on the tagging path never fires, and a caller cannot distinguish "this string is not an ARN" from a modeled input-constraint violation; error-classification and branch-on-error-code logic takes the wrong path. Because the expected message member is absent entirely, code that reads the error text off the response also gets nothing, so neither the code nor the message is usable.

  Fix: the ListTagsForResource handler's ResourceArn parsing (the shared tagging-path ARN parse that also fronts TagResource/UntagResource) must raise the tagging layer's 400 BadRequestException with the fixed lowercase `message` on a syntax failure, before/instead of delegating to the generic request-validation message builder.

  Traces: pca-connector-scep-error-validation-params#task#3.

Related but NOT covering this cause: open issue 20261029T190804Z-pca-connector-scep-pattern-constraint-validationexception-messages-use-the-wrong-form.md describes this same divergence in prose (its cause 5), but it carries NO replay key for this step — its Issue-Key lines cover only CreateConnector, CreateChallenge, ListConnectors and GetConnector, and the replay confirms this cluster is unowned (owned_by empty) while the ListConnectors/GetConnector clusters in the same fixture report that issue as their owner. A replay of that issue therefore never exercises ListTagsForResource, so this cause needs its own gated key. Open issue 20261030T160351Z-pca-connector-scep-untagresource-on-an-absent-resource-answers-404-resourcenotfoundexception.md concerns the ABSENT-resource branch of the tagging paths (400 NotFoundException "Resource was not found"), which is a different branch from this ARN-syntax refusal.

## Evidence
Replay against trace set simparity-wire-aws-v1 (emulator up, 16 steps, 13 passed, 3 mismatched, score 81.2%). Of the 3 mismatch clusters, 2 (ListConnectors task#1, GetConnector task#2) carry owned_by = 20261029T190804Z-pca-connector-scep-pattern-constraint-validationexception-messages-use-the-wrong-form.md and are therefore not re-filed. The 1 net-new cluster (stub_path empty, owned_by empty) is:

- pca-connector-scep-error-validation-params#task#3 ListTagsForResource: status expected 400, got 400; $.message: missing (expected 'Invalid input resource arn: bad syntax', got None); $headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'ValidationException').
  divergence_key: service=pca-connector-scep|ListTagsForResource|status=400->400|keys=$.message,$headers.x-amzn-errortype

Requirements: HTTP 400; response header x-amzn-errortype = `BadRequestException`; body member `message` (lowercase) with the value "Invalid input resource arn: bad syntax". That whole message is fixed text — there is no variable part, so no ARN, member name or regex may be interpolated into it. The particular rejected input in this recording is a per-run value and is not part of the requirement; neither is any request id or host in the recorded response.

## Steps to Reproduce
1. Boot the emulator and replay fixture pca-connector-scep-error-validation-params from trace set simparity-wire-aws-v1.
2. Step task#3 issues GET /tags/{ResourceArn} where ResourceArn is a string that is not a syntactically valid ARN.
3. Expect HTTP 400 with header x-amzn-errortype: BadRequestException and body {"message": "Invalid input resource arn: bad syntax"}.
4. Observe HTTP 400 with x-amzn-errortype: ValidationException and no lowercase `message` member carrying that text — the emulator emits a synthesized pattern-constraint message for ResourceArn, which has no @pattern in the model.
5. Contrast with task#1 (ListConnectors, NextToken) and task#2 (GetConnector, ConnectorArn) in the same fixture, where members DO carry a @pattern and ValidationException is the correct error code.
