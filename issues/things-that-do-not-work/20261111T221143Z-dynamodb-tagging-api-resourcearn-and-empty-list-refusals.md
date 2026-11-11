Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-tag-api-resource-arn-validation-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-tag-api-resource-arn-validation-rejected dynamodb-tag-untag-resource-list-not-null-and-empty-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TagResource|4xx->4xx|$.message
Issue-Key: dynamodb|UntagResource|4xx->4xx|$.message
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.message
Issue-Related: 20261111T221143Z-dynamodb-control-plane-and-tagging-operations-answer-validationexception.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: tagging-API ResourceArn and empty-list refusals use the wrong message and error class

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The tagging API's input validation reports the wrong thing twice over: a malformed ResourceArn is reported as a null-member constraint violation, and an empty Tags/TagKeys list is reported as a resource-not-found. 5 causes.

Cause 1 of 5: TagResource's malformed-ARN refusal uses AWS's generic null-constraint envelope instead of the ARN sentence.
AWS: recorded behavior — status 400 ValidationException with message 'Invalid TableArn'. That is the whole message: fixed text, no variable part, and it does not name a member path or echo the value. (Smithy: TagResource.ResourceArn is @required, target ResourceArnString, string @length 1..1283 — the model states the length bound but not the ARN-shape rule, which AWS enforces in the service.)
Emulator: status 400 ValidationException with "1 validation error detected: Value null at 'resourceArn' failed to satisfy constraint: Member must not be null" — AWS's null-member envelope, for an ARN that was present but malformed.
Why: the message tells the client the ARN was absent when it was supplied and merely wrong, so an operator chases a missing parameter instead of a bad ARN; tooling that greps for 'Invalid TableArn' never matches.
Fix: TagResource's ResourceArn validation — distinguish absent from malformed and emit 'Invalid TableArn' for the latter.
Traces: dynamodb-tag-api-resource-arn-validation-rejected#task#1.

Cause 2 of 5: the same defect on UntagResource.
AWS: as cause 1 — 400 ValidationException, message 'Invalid TableArn'. (Smithy: UntagResource.ResourceArn @required, ResourceArnString.)
Emulator: the same null-constraint envelope naming 'resourceArn'.
Why: as cause 1.
Fix: as cause 1, on UntagResource.
Traces: dynamodb-tag-api-resource-arn-validation-rejected#task#5.

Cause 3 of 5: the same defect on ListTagsOfResource.
AWS: as cause 1 — 400 ValidationException, message 'Invalid TableArn'.
Emulator: the same null-constraint envelope naming 'resourceArn'.
Why: as cause 1.
Fix: as cause 1, on ListTagsOfResource. Since all three share the member, the ARN check belongs in the shared tagging-input validation.
Traces: dynamodb-tag-api-resource-arn-validation-rejected#task#9.

Cause 4 of 5: TagResource with an empty Tags list answers ResourceNotFoundException instead of ValidationException.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Atleast one Tag needs to be provided as Input.' (AWS's literal wording, including the run-together 'Atleast' and the trailing period; no variable part). The input check runs before the resource lookup. Smithy: TagResource.Tags is @required with target TagList and no modeled @length min, so the non-empty rule is service-enforced.
Emulator: status 400 with __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' and message 'Requested resource not found: ResourceArn: <arn> not found' — it resolved the ARN first and reported the lookup failure, hiding the real defect.
Why: the client is told its table does not exist when the table is irrelevant and the request body is empty; the error class is wrong (not-found vs validation), so retry/creation logic takes the wrong branch and may try to create a table.
Fix: TagResource's validation ordering — validate Tags non-empty before resolving ResourceArn, and raise ValidationException with AWS's sentence.
Traces: dynamodb-tag-untag-resource-list-not-null-and-empty-rejected#task#2.

Cause 5 of 5: UntagResource with an empty TagKeys list, same inversion.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.validate#ValidationException', message 'Atleast one Tag Key needs to be provided as Input.' (literal wording, no variable part). Smithy: UntagResource.TagKeys is @required, target TagKeyList.
Emulator: status 400 ResourceNotFoundException with 'Requested resource not found: ResourceArn: <arn> not found'.
Why: as cause 4.
Fix: as cause 4, on UntagResource.
Traces: dynamodb-tag-untag-resource-list-not-null-and-empty-rejected#task#5.

Related but not covering: open issue 20261111T170613Z-dynamodb-control-plane-refusals-use-emulator-authored-text.md covers other control-plane refusal text including a valid stream ARN being rejected by tagging; it does not carry these clusters' replay keys and does not cover the 'Invalid TableArn' substitution or the empty-list/not-found inversion.

## Evidence
Causes 1-3 (TagResource / UntagResource / ListTagsOfResource), status 400 both sides: $.message expected 'Invalid TableArn', got "1 validation error detected: Value null at 'resourceArn' failed to satisfy constraint: Member must not be null".
Cause 4 (TagResource), status 400 both sides: $.__type expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException'; $.message expected 'Atleast one Tag needs to be provided as Input.', got 'Requested resource not found: ResourceArn: <arn> not found'.
Cause 5 (UntagResource), status 400 both sides: $.__type expected 'com.amazon.coral.validate#ValidationException', got 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException'; $.message expected 'Atleast one Tag Key needs to be provided as Input.', got 'Requested resource not found: ResourceArn: <arn> not found'.

## Steps to Reproduce
1. Replay dynamodb-tag-api-resource-arn-validation-rejected#task#1, #task#5 and #task#9: each must answer 400 ValidationException with the exact message 'Invalid TableArn'.
2. Replay dynamodb-tag-untag-resource-list-not-null-and-empty-rejected#task#2: must answer 400 com.amazon.coral.validate#ValidationException with 'Atleast one Tag needs to be provided as Input.' — not a ResourceNotFoundException.
3. Replay the same fixture's #task#5: must answer 400 ValidationException with 'Atleast one Tag Key needs to be provided as Input.'
