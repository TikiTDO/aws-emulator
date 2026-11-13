Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-workload-create-validation-envelopes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-workload-create-validation-envelopes wellarchitected-custom-lens-import-version-lifecycle wellarchitected-lens-review-answer-lifecycle wellarchitected-milestone-lifecycle wellarchitected-review-template-lifecycle wellarchitected-lens-association-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateWorkload|4xx->4xx|$.Reason
Issue-Key: wellarchitected|ImportLens|4xx->4xx|$.Message
Issue-Key: wellarchitected|ImportLens|4xx->4xx|$.Reason
Issue-Key: wellarchitected|DeleteLens|4xx->4xx|$.Fields
Issue-Key: wellarchitected|DeleteLens|4xx->4xx|$.Message
Issue-Key: wellarchitected|DeleteLens|4xx->4xx|$.Reason
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateMilestone|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->4xx|$.Message
Issue-Key: wellarchitected|DisassociateLenses|4xx->4xx|$.Fields[*]
Issue-Key: wellarchitected|DisassociateLenses|4xx->4xx|$.Message
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# ValidationException envelopes use generic framework text and omit Reason and Fields

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkload

## Description
This bundle has 8 causes. Every one is a 400 ValidationException whose status is right but whose envelope is wrong: the emulator renders a generic 'N validation error detected: Value <v> at <member> failed to satisfy constraint: ...' string instead of the wording Well-Architected uses for that constraint class, and it leaves ValidationException$Reason and ValidationException$Fields unset where AWS populates them. Smithy: ValidationException{Message (required), Reason: ValidationExceptionReason enum [UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER], Fields: list<ValidationExceptionField{Name, Message}>}, httpError 400.

Cause 1 of 8: CreateWorkload length violation message.
AWS: CreateWorkload$WorkloadName is @length min 3 max 100. A too-short value is reported with the JSON-schema style text '[Validation] [string "<value>" is too short (length: <actualLength>, required minimum: <min>)]' — fixed text apart from <value> (the rejected value from the request), <actualLength> (its length) and <min> (the member's modeled minimum).
Emulator: '[Validation] 1 validation error detected: Value '<value>' at 'workloadName' failed to satisfy constraint: Member must have length greater than or equal to 3'.
Why: a client that matches on AWS's validation text, or surfaces it to an operator, sees a message AWS never emits.
Fix: the shared validation-message renderer for @length violations in this crate's validation layer.
Traces: wellarchitected-workload-create-validation-envelopes#task#1.

Cause 2 of 8: CreateWorkload missing-required-member message, and Reason is unset.
AWS: omitting the required ReviewOwner is reported as '[Validation] [object has missing required properties (["<MemberName>"])]' — fixed except <MemberName>, the modeled member name as it appears on the wire — and ValidationException$Reason is emitted (empty in the recording) rather than absent.
Emulator: '[Validation] 1 validation error detected: Value null at 'ReviewOwner' failed to satisfy constraint: Member must not be null' and no Reason member at all.
Why: wrong text, plus a client reading Reason off the error dereferences a missing member.
Fix: the missing-required-member renderer and the ValidationException serializer (always emit Reason).
Traces: wellarchitected-workload-create-validation-envelopes#task#3.

Cause 3 of 8: ImportLens unparsable document message and Reason.
AWS: a JSONString that is not parsable JSON is rejected with the fixed message '[Validation] Invalid JSONString' — no variable part, and in particular no parser diagnostic — and Reason is emitted empty rather than 'CANNOT_PARSE'.
Emulator: '[Validation] Value at JSONString failed to satisfy constraint: the lens document is not JSON: <parser diagnostic>' with Reason 'CANNOT_PARSE'.
Why: the emulator leaks its own JSON parser's diagnostic into an API contract, and Reason disagrees with AWS, so a client branching on Reason takes a different path.
Fix: the ImportLens handler's document-parse error path.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#7 (cluster also covers task#8 of the same fixture).

Cause 4 of 8: DeleteLens invalid LensStatus message, with Reason and Fields unset.
AWS: an out-of-range LensStatus query value is reported as a field validation failure: Message '[Validation] The LensStatusType that was passed in is invalid.' (fixed text), Reason 'FIELD_VALIDATION_FAILED', and Fields with one entry {Name 'LensStatusType', Message 'The LensStatusType that was passed in is invalid.'}.
Emulator: the generic enum text ('1 validation error detected: Value '<v>' at 'lensStatus' failed to satisfy constraint: Member must satisfy enum value set: [...]') with no Reason and no Fields.
Why: a client that reads Fields to point an operator at the offending member gets nothing.
Fix: the enum-validation path for DeleteLens plus the ValidationException serializer (emit Reason and Fields).
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#9.

Cause 5 of 8: UpdateAnswer invalid Reason enum message.
AWS: an out-of-range AnswerReason is reported in the JSON-schema enum form '[Validation] [instance value ("<value>") not found in enum (possible values: ["OUT_OF_SCOPE","BUSINESS_PRIORITIES","ARCHITECTURE_CONSTRAINTS","OTHER","NONE"])]' — fixed apart from <value> from the request and the modeled enum value list.
Emulator: the generic '1 validation error detected: Value '<value>' at 'reason' failed to satisfy constraint: Member must satisfy enum value set: [...]' form.
Why: wrong wording for the enum class, in a path clients commonly show verbatim.
Fix: the enum-violation renderer in the shared validation layer.
Traces: wellarchitected-lens-review-answer-lifecycle#task#10.

Cause 6 of 8: CreateMilestone length violation message.
AWS: CreateMilestone$MilestoneName is @length min 3 max 100; a too-short value is reported in the same '[Validation] [string "<value>" is too short (length: <actualLength>, required minimum: <min>)]' form as cause 1.
Emulator: the generic 'Member must have length greater than or equal to 3' form naming 'milestoneName'.
Why/Fix: as cause 1 — the same renderer, confirming it is shared and not a per-operation slip.
Traces: wellarchitected-milestone-lifecycle#task#7.

Cause 7 of 8: CreateReviewTemplate pattern violation message.
AWS: CreateReviewTemplate$TemplateName is @pattern "^[A-Za-z0-9-_.,:/()@!&?#+'\u2019\\s]+$"; a non-matching value is reported as '[Validation] [ECMA 262 regex "<pattern>" does not match input string "<value>"]' — fixed apart from the modeled pattern and the rejected value from the request.
Emulator: '1 validation error detected: Value '<value>' at 'templateName' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'.
Why/Fix: the @pattern violation renderer in the shared validation layer.
Traces: wellarchitected-review-template-lifecycle#task#5.

Cause 8 of 8: DisassociateLenses refusal of the default lens names the wrong field and wrong message.
AWS: disassociating the Well-Architected Framework lens is refused 400 with Message '[Validation] Cannot disassociate default lens: <lensAlias>.' and Fields with one entry {Name 'Lenses', Message 'Cannot disassociate default lens: <lensAlias>.'} — fixed text apart from <lensAlias>, the default lens alias from the request.
Emulator: Fields[0] is {Name 'LensAliases', Message 'Value at LensAliases failed to satisfy constraint: must not name the Well-Architected Framework lens (wellarchitected), which cannot be removed from a workload'} and Message repeats that text.
Why: the field name a client uses to attribute the error ('Lenses') is wrong, and the message is the emulator's own wording.
Fix: the DisassociateLenses handler's default-lens refusal.
Traces: wellarchitected-lens-association-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1, 8 mismatch clusters, all service=wellarchitected, all expected 400 / actual 400 unless noted.

Cause 1 — wellarchitected-workload-create-validation-envelopes#task#1, CreateWorkload: $.Message expected '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]', got "[Validation] 1 validation error detected: Value 'ab' at 'workloadName' failed to satisfy constraint: Member must have length greater than or equal to 3". Cluster also covers task#2, task#4, task#5 of the same fixture.

Cause 2 — wellarchitected-workload-create-validation-envelopes#task#3, CreateWorkload: $.Message expected '[Validation] [object has missing required properties (["ReviewOwner"])]', got "[Validation] 1 validation error detected: Value null at 'ReviewOwner' failed to satisfy constraint: Member must not be null"; $.Reason expected '' (present), got absent.

Cause 3 — wellarchitected-custom-lens-import-version-lifecycle#task#7, ImportLens: $.Message expected '[Validation] Invalid JSONString', got '[Validation] Value at JSONString failed to satisfy constraint: the lens document is not JSON: key must be a string at line 1 column 2'; $.Reason expected '', got 'CANNOT_PARSE'. Cluster also covers task#8.

Cause 4 — wellarchitected-custom-lens-import-version-lifecycle#task#9, DeleteLens: $.Fields expected [{'Name': 'LensStatusType', 'Message': 'The LensStatusType that was passed in is invalid.'}], got absent; $.Message expected '[Validation] The LensStatusType that was passed in is invalid.', got "[Validation] 1 validation error detected: Value 'SIM2REAL' at 'lensStatus' failed to satisfy constraint: Member must satisfy enum value set: [ALL, DRAFT, PUBLISHED]"; $.Reason expected 'FIELD_VALIDATION_FAILED', got absent.

Cause 5 — wellarchitected-lens-review-answer-lifecycle#task#10, UpdateAnswer: $.Message expected '[Validation] [instance value ("SIM2REAL_PROBE") not found in enum (possible values: ["OUT_OF_SCOPE","BUSINESS_PRIORITIES","ARCHITECTURE_CONSTRAINTS","OTHER","NONE"])]', got "[Validation] 1 validation error detected: Value 'SIM2REAL_PROBE' at 'reason' failed to satisfy constraint: Member must satisfy enum value set: [OUT_OF_SCOPE, BUSINESS_PRIORITIES, ARCHITECTURE_CONSTRAINTS, OTHER, NONE]".

Cause 6 — wellarchitected-milestone-lifecycle#task#7, CreateMilestone: $.Message expected '[Validation] [string "ab" is too short (length: 2, required minimum: 3)]', got "[Validation] 1 validation error detected: Value 'ab' at 'milestoneName' failed to satisfy constraint: Member must have length greater than or equal to 3".

Cause 7 — wellarchitected-review-template-lifecycle#task#5, CreateReviewTemplate: $.Message expected '[Validation] [ECMA 262 regex "^[A-Za-z0-9-_.,:/()@!&?#+\'\u2019\\s]+$" does not match input string "sim2real*probe*tpl"]', got "[Validation] 1 validation error detected: Value 'sim2real*probe*tpl' at 'templateName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-_.,:/()@!&?#+'\u2019\\s]+$".

Cause 8 — wellarchitected-lens-association-lifecycle#task#7, DisassociateLenses: $.Fields[0] expected {'Name': 'Lenses', 'Message': 'Cannot disassociate default lens: wellarchitected.'}, got {'Name': 'LensAliases', 'Message': 'Value at LensAliases failed to satisfy constraint: must not name the Well-Architected Framework lens (wellarchitected), which cannot be removed from a workload'}; $.Message expected '[Validation] Cannot disassociate default lens: wellarchitected.', got the emulator's LensAliases text.

Smithy: ValidationException{Message required, Reason: ValidationExceptionReason [UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER], Fields: list<ValidationExceptionField{Name, Message}>}, httpError 400; CreateWorkload$WorkloadName @length 3..100, CreateWorkload$ReviewOwner, Lenses/Description/Environment required; CreateMilestone$MilestoneName @length 3..100; CreateReviewTemplate$TemplateName @length 3..100 @pattern ^[A-Za-z0-9-_.,:/()@!&?#+'\u2019\s]+$; DeleteLens$LensStatus -> LensStatusType enum; UpdateAnswer$Reason -> AnswerReason enum.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. POST /workloads with WorkloadName 'ab' (cause 1) and with ReviewOwner omitted (cause 2) and read $.Message and $.Reason.
4. PUT /importLens with a body that is not parsable JSON (cause 3).
5. DELETE /lenses/{alias}?LensStatus=<not in [ALL,DRAFT,PUBLISHED]> and read $.Message, $.Reason, $.Fields (cause 4).
6. PATCH /workloads/{id}/lensReviews/wellarchitected/answers/{q} with Reason outside the AnswerReason enum (cause 5).
7. POST /workloads/{id}/milestones with MilestoneName 'ab' (cause 6).
8. POST /reviewTemplates with TemplateName 'sim2real*probe*tpl' (cause 7).
9. PATCH /workloads/{id}/disassociateLenses with LensAliases ['wellarchitected'] and read $.Fields[0].Name (cause 8).
