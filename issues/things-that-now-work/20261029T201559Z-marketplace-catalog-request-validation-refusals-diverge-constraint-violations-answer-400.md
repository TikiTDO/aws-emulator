Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-catalog-describe-assessment-identifier-validation-boundary
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-catalog-describe-assessment-identifier-validation-boundary marketplace-catalog-list-entities-sorted-then-describe-first-v2 marketplace-catalog-list-entities-validation-errors-v2 marketplace-catalog-list-tags-for-resource-arn-validation-boundary marketplace-catalog-start-changeset-validation-error-v2
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog|DescribeAssessment|4xx->4xx|$.Message
Issue-Key: marketplace-catalog|ListEntities|4xx->4xx|$.Message
Issue-Key: marketplace-catalog|ListEntities|4xx->4xx|status
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: marketplace-catalog|ListTagsForResource|4xx->4xx|status
Issue-Key: marketplace-catalog|StartChangeSet|4xx->4xx|$.Message
Issue-Key: marketplace-catalog|StartChangeSet|4xx->4xx|status
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# Request-validation refusals diverge: constraint violations answer 400 with no Message instead of 422 ValidationException, and two 422 texts are misworded

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEntities, StartChangeSet, ListTagsForResource, DescribeAssessment

## Description
The emulator's input-validation layer for AWS Marketplace Catalog diverges from AWS in two ways. (a) For three constraint classes the request is refused by the generic front-door with HTTP 400 and a body carrying no `Message` member at all, where AWS answers its modelled 422 ValidationException with a specific sentence. (b) Where the emulator DOES answer the right 422, two of its sentences are rendered from the emulator's own data (the Smithy pattern literal, the crate's per-entity-type sort-attribute table) instead of the fixed text AWS prints. All five causes are one family: the service's validation sentences and their status are not reproduced. 5 causes, all on marketplace-catalog, all must be closed before submitting.

For every ValidationException below, AWS wraps the sentence in square brackets inside `Message` (the service reports a list of validation findings), the sentence's wording and its trailing regex/limit/allowed-value text are FIXED, and only the quoted offending value is taken from the request. Note the resolved issue 20261022T124555Z-marketplace-catalog-request-validation-... touched request validation; it is closed and does not cover any of these, and the open issues listed under this service concern unrelated handler semantics (change-set uniqueness, pagination tokens, BatchDescribeEntities keys).

Cause 1 of 5: DescribeAssessment AssessmentIdentifier pattern message prints the model's regex spelling, not AWS's.
  AWS: Smithy: DescribeAssessment.AssessmentIdentifier is @required with @length(1..2048) and @pattern "^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$"; modelled error ValidationException @httpError(422). The recorded response is 422 with Message "[Requested assessment identifier '<identifier>' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]". Fixed text: everything except the quoted identifier, INCLUDING the regex literal, which AWS spells as two separately anchored alternatives joined by `|` and with `::` in the ARN branch. Variable part: the identifier, echoed from the request.
  Emulator: answers 422 (status correct) but renders the regex straight from the Smithy @pattern: "... It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.]" — one anchored group instead of two anchored alternatives, and `:[0-9]*:` instead of `::`.
  Why: a client (or a test) that asserts on the validation text, or that parses the advertised regex to pre-validate identifiers locally, gets a different pattern than the service publishes; the emulator's spelling also advertises an account-id segment AWS's message does not.
  Fix: the DescribeAssessment identifier validation path — the message template must carry AWS's literal regex string as fixed text rather than interpolating the model's @pattern.
  Traces: marketplace-catalog-describe-assessment-identifier-validation-boundary#task#1.

Cause 2 of 5: ListEntities invalid Sort.SortBy message lists the emulator's own sort attributes instead of AWS's allowed-value text.
  AWS: Smithy: ListEntities.Sort -> Sort$SortBy (string, no enum constraint in the model), modelled error ValidationException @httpError(422) — the allowed-value set is not modelled, so this is recorded behavior only: AWS answers 422 with Message "[Requested sort by '<value>' is invalid. It should be one of '[]'.]", i.e. the allowed-value list is rendered as an empty bracketed list for this request. Fixed text: the whole sentence including the `'[]'` rendering; variable part: the submitted SortBy value.
  Emulator: answers 422 with "[Requested sort by 'NotARealSortField' is invalid. It should be one of 'LastModifiedDate, EntityId, ProductTitle, Visibility'.]" — it substitutes the crate's hand-maintained per-entity-type sort-attribute table into the allowed-value slot.
  Why: the emulator publishes an allowed-value set the service does not publish in this refusal, so a client that discovers valid sort fields from the error text is led to a list AWS never returned, and assertions on the refusal text fail.
  Fix: the ListEntities Sort validation path — the allowed-value slot of the message must be produced the way the service produces it, not from the emulator's sort-attribute constant.
  Traces: marketplace-catalog-list-entities-sorted-then-describe-first-v2#task#2.

Cause 3 of 5: ListEntities out-of-range MaxResults is refused 400 with no Message instead of 422 ValidationException.
  AWS: Smithy: ListEntities.MaxResults targets ListEntitiesMaxResultInteger with @range(min 1, max 50); modelled error ValidationException @httpError(422). Recorded: 422 with Message "[Provided max results value '<n>' is out of bounds. It should be between 1 and 50.]". Fixed text: "Provided max results value ... is out of bounds. It should be between 1 and 50." — the bounds are the member's modelled range; variable part: the submitted value.
  Emulator: HTTP 400 and the response body has no `Message` member at all (replay reports `$.Message: missing`), i.e. the out-of-range value is rejected generically before the operation's own validation runs.
  Why: an SDK maps 422+ValidationException to the modelled exception type clients catch; a bare 400 with no Message surfaces as an unmodelled/unknown error with no text, so retry and error-handling branches keyed on ValidationException never fire and the client cannot tell the operator which parameter was wrong.
  Fix: ListEntities' input validation — the page-size range check must raise the service's ValidationException (422) with the out-of-bounds sentence, not fall through to the generic 400 front-door rejection.
  Traces: marketplace-catalog-list-entities-validation-errors-v2#task#2.

Cause 4 of 5: ListTagsForResource malformed ResourceArn is refused 400 with no Message instead of 422 ValidationException.
  AWS: Smithy: ListTagsForResource.ResourceArn is @required, targets ResourceARN with @length(1..255) and @pattern "^arn:[\\w+=/,.@-]+:aws-marketplace:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$"; modelled error ValidationException @httpError(422). Recorded: 422 with Message "[Requested resource arn '<arn>' is invalid. It should match with ^arn:[\\w+=/,.@-]+:aws-marketplace:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$.]". Fixed text: the sentence plus the regex literal; variable part: the submitted ARN.
  Emulator: HTTP 400 with no `Message` member — the pattern violation is rejected by the generic front door instead of the operation's validation.
  Why: same as cause 3 — the modelled ValidationException is unreachable for a malformed ARN, and the client gets no indication that `ResourceArn` is the bad member.
  Fix: ListTagsForResource input validation — enforce ResourceArn's pattern inside the handler's validation and refuse with 422 ValidationException carrying the invalid-resource-arn sentence.
  Traces: marketplace-catalog-list-tags-for-resource-arn-validation-boundary#task#2.

Cause 5 of 5: StartChangeSet entity type without the `Type@Version` form is refused 400 with no Message instead of 422 ValidationException.
  AWS: Smithy: StartChangeSet.ChangeSet -> Change$Entity -> Entity$Type (@required, @length(1..255)); the `Type@Version` requirement is not expressed as a @pattern on the shape, so this is recorded behavior only: AWS answers 422 ValidationException with Message "[Requested entity type '<type>' is invalid. It should match with ^(.*)@(.*)$.]". Fixed text: the sentence and the `^(.*)@(.*)$` literal; variable part: the submitted entity type.
  Emulator: HTTP 400 with no `Message` member; the versionless entity type is rejected generically, so neither the 422 class nor the sentence naming the required `Type@Version` shape is produced.
  Why: StartChangeSet is the service's only mutating entry point; a caller that omits the entity-type version gets an untyped, textless 400 and no hint that the type must be qualified with `@<version>`, and code catching ValidationException does not catch it.
  Fix: StartChangeSet's change validation — validate Entity$Type against the qualified-type form and refuse with the service's 422 ValidationException and its invalid-entity-type sentence.
  Traces: marketplace-catalog-start-changeset-validation-error-v2#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (18 fixtures, 38 steps, 33 passed, 5 mismatched, score 86.8%). All five mismatches are marketplace-catalog; none carried stub_path or owned_by.

Cause 1 — marketplace-catalog-describe-assessment-identifier-validation-boundary#task#1, DescribeAssessment: status expected 422, got 422; $.Message value_mismatch.
  expected: "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]"
  actual:   "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.]"
  (the quoted identifier is the request's value; the divergence is entirely in the fixed regex text)

Cause 2 — marketplace-catalog-list-entities-sorted-then-describe-first-v2#task#2, ListEntities: status expected 422, got 422; $.Message value_mismatch.
  expected: "[Requested sort by 'NotARealSortField' is invalid. It should be one of '[]'.]"
  actual:   "[Requested sort by 'NotARealSortField' is invalid. It should be one of 'LastModifiedDate, EntityId, ProductTitle, Visibility'.]"

Cause 3 — marketplace-catalog-list-entities-validation-errors-v2#task#2, ListEntities: status expected 422, got 400; $.Message missing.
  expected Message: "[Provided max results value '999' is out of bounds. It should be between 1 and 50.]"; actual: no Message member in the body.

Cause 4 — marketplace-catalog-list-tags-for-resource-arn-validation-boundary#task#2, ListTagsForResource: status expected 422, got 400; $.Message missing.
  expected Message: "[Requested resource arn 'arn:aws:aws-marketplace' is invalid. It should match with ^arn:[\\w+=/,.@-]+:aws-marketplace:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*$.]"; actual: no Message member.

Cause 5 — marketplace-catalog-start-changeset-validation-error-v2#task#1, StartChangeSet: status expected 422, got 400; $.Message missing.
  expected Message: "[Requested entity type 'AmiProduct' is invalid. It should match with ^(.*)@(.*)$.]"; actual: no Message member.

Model evidence (services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json): every one of DescribeAssessment, ListEntities, ListTagsForResource and StartChangeSet binds ValidationException with @httpError(422); ListEntities.MaxResults has @range(1..50); ListTagsForResource.ResourceArn has the aws-marketplace ARN @pattern; DescribeAssessment.AssessmentIdentifier has the assessment-id/ARN @pattern quoted above.

## Steps to Reproduce
Boot the emulator from the checkout and replay the wire corpus (replay_wire_corpus against trace set simparity-wire-aws-v1); the five steps below are the failing ones.

1. Cause 1: POST /DescribeAssessment with Catalog=AWSMarketplace and AssessmentIdentifier="not-a-valid-identifier" (a value matching neither the assessment-<id> nor the Assessment ARN form). Fixture marketplace-catalog-describe-assessment-identifier-validation-boundary, step task#1. Compare the 422 body's Message text against the recorded one.
2. Cause 2: POST /ListEntities with a valid Catalog/EntityType and Sort={SortBy:"NotARealSortField"}. Fixture marketplace-catalog-list-entities-sorted-then-describe-first-v2, step task#2.
3. Cause 3: POST /ListEntities with a valid Catalog/EntityType and MaxResults=999 (above the modelled max of 50). Fixture marketplace-catalog-list-entities-validation-errors-v2, step task#2. Observe HTTP 400 and a body with no Message instead of 422 ValidationException.
4. Cause 4: POST /ListTagsForResource with ResourceArn="arn:aws:aws-marketplace" (a truncated ARN that fails the member's pattern). Fixture marketplace-catalog-list-tags-for-resource-arn-validation-boundary, step task#2. Observe HTTP 400 with no Message.
5. Cause 5: POST /StartChangeSet with a change whose Entity.Type is an unqualified "AmiProduct" (no @<version> suffix). Fixture marketplace-catalog-start-changeset-validation-error-v2, step task#1. Observe HTTP 400 with no Message instead of 422 ValidationException.
