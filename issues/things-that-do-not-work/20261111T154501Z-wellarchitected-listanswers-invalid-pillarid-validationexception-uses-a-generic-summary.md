Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-pagination-and-limit-validation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|ListAnswers|4xx->4xx|$.Fields[*].Message
Issue-Key: wellarchitected|ListAnswers|4xx->4xx|$.Message
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# ListAnswers invalid-PillarId ValidationException uses a generic summary Message and emulator-authored field text

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAnswers

## Description
This bundle has 1 cause.

Cause 1 of 1: ListAnswers rejects an out-of-lens PillarId with the right status (400 ValidationException) but the wrong envelope body: the summary Message is a generic placeholder instead of the field's own diagnostic, and the per-field diagnostic is the emulator's own wording, which enumerates the emulator's internal pillar table.

AWS: Smithy (com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda): ListAnswers GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers, PillarId is an httpQuery member targeting PillarId with @length min 1 max 64 and no modeled enum, errors include ValidationException (errorKind client, httpError 400). ValidationException is {Message (required), Reason: ValidationExceptionReason [UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER], Fields: list<ValidationExceptionField{Name, Message}>}. The model fixes neither the text of a field-validation message nor the set of valid pillar ids, so for the wording this is recorded behavior only: every field-validation envelope recorded from AWS in this corpus sets Message to the literal prefix '[Validation] ' followed by exactly the Fields[0].Message diagnostic, never a generic summary — e.g. the recorded DeleteLens envelope in the same corpus is Message '[Validation] The LensStatusType that was passed in is invalid.' alongside Fields [{Name 'LensStatusType', Message 'The LensStatusType that was passed in is invalid.'}], and the recorded DisassociateLenses envelope is Message '[Validation] Cannot disassociate default lens: wellarchitected.' alongside the identical Fields[0].Message. So the requirement has two fixed parts and one variable part: the fixed prefix '[Validation] ', the fixed rule that the remainder equals Fields[0].Message verbatim, and the field-level diagnostic itself, whose variable parts are the rejected member name (here 'PillarId', taken from the request binding) and the rejected value from the request. Hypothesis: following the LensStatusType form AWS words the pillar diagnostic as 'The PillarId that was passed in is invalid.' and does not list the accepted pillar ids; the corpus step grades this field as shaped rather than asserting the literal string, so the exact AWS sentence is not recorded and only the envelope rule above is a hard requirement. Reason should be FIELD_VALIDATION_FAILED for this class, as the recorded DeleteLens field-validation envelope shows.

Emulator: returns 400 with $.Message '[Validation] The validation of one or more fields has failed.' — a generic summary that names neither the member nor the value — and $.Fields[0].Message 'PillarId contains an invalid pillar. Valid pillars are: [operationalExcellence, security, reliability, performance, costOptimization, sustainability].', which is the emulator's own sentence plus a dump of its built-in pillar list. Both failed the step's shape assertion on those two paths.

Why: a client that reads ValidationException$Message to attribute the failure (log it, show it to an operator, or match on it to retry with a corrected PillarId) gets a sentence that identifies nothing, because AWS puts the actionable diagnostic in Message as well as in Fields. A client that matches the field diagnostic sees emulator-authored text, and the appended 'Valid pillars are: [...]' list is a contract AWS never publishes in this error, so anything that parses it is parsing a value that only exists in the emulator.

Fix: the ListAnswers handler's PillarId validation path and the shared ValidationException field-envelope renderer in this crate's validation layer — the renderer must set Message to '[Validation] ' + the field diagnostic rather than a generic summary, and set Reason to FIELD_VALIDATION_FAILED, while the ListAnswers path must word the pillar diagnostic as AWS does instead of enumerating its internal pillar table.

Related open issue: 20261029T192731Z-wellarchitected-validationexception-envelopes-use-generic-framework-text-and-omit.md covers the same ValidationException envelope family for CreateWorkload, ImportLens, DeleteLens, UpdateAnswer, CreateMilestone, CreateReviewTemplate and DisassociateLenses. It does NOT cover this cluster: this one is ListAnswers/PillarId (cluster key wellarchitected|ListAnswers|status=400->400|keys=$.Fields[0].Message,$.Message) and the replay reports it unowned, so it needs its own replay key.

Traces: wellarchitected-pagination-and-limit-validation#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 94 steps, 74 passed, 20 mismatched, score 78.7%). 1 mismatch cluster, service=wellarchitected, operation ListAnswers, verdict mismatch, expected status 400 / actual status 400, stub_path empty, owned_by empty.

Cause 1 — wellarchitected-pagination-and-limit-validation#task#6, ListAnswers (GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers?PillarId=<not a pillar of the lens>):
  $.Message: value_mismatch — expected '<shaped-not-asserted>' (the step grades this field's shape, not a literal), got '[Validation] The validation of one or more fields has failed.'
  $.Fields[0].Message: value_mismatch — expected '<shaped-not-asserted>', got 'PillarId contains an invalid pillar. Valid pillars are: [operationalExcellence, security, reliability, performance, costOptimization, sustainability].'

Fixed vs variable in the required message: '[Validation] ' is fixed text; the remainder must be the Fields[0] diagnostic verbatim (fixed rule); within that diagnostic the member name ('PillarId') comes from the request's query binding and the rejected value comes from the request. The enumerated pillar list in the emulator's text is emulator state, not part of any AWS template, and the recorded 'Valid pillars' wording appears nowhere in the AWS recording.

Corroborating recorded AWS envelopes of the same class in this corpus (both 400 ValidationException): wellarchitected-custom-lens-import-version-lifecycle#task#9 DeleteLens — Message '[Validation] The LensStatusType that was passed in is invalid.', Reason 'FIELD_VALIDATION_FAILED', Fields [{'Name': 'LensStatusType', 'Message': 'The LensStatusType that was passed in is invalid.'}]; wellarchitected-lens-association-lifecycle#task#7 DisassociateLenses — Message '[Validation] Cannot disassociate default lens: wellarchitected.' with Fields[0].Message identical to the sentence after the prefix.

Smithy: ListAnswers input PillarId @length 1..64 (httpQuery 'PillarId', no enum), errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; ValidationException{Message required, Reason enum [UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER], Fields list<ValidationExceptionField{Name, Message}>}.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 and look at fixture wellarchitected-pagination-and-limit-validation, step task#6.
3. Create a workload and associate the wellarchitected lens (steps earlier in the same fixture do this).
4. GET /workloads/{WorkloadId}/lensReviews/wellarchitected/answers?PillarId=<a value that is not a pillar of the lens>.
5. Observe HTTP 400 ValidationException and read $.Message, $.Fields[0].Name, $.Fields[0].Message and $.Reason.
6. Expected: Message is '[Validation] ' followed verbatim by the Fields[0] diagnostic naming the rejected member and value, with Reason 'FIELD_VALIDATION_FAILED'. Actual: Message is the generic '[Validation] The validation of one or more fields has failed.' and Fields[0].Message is the emulator's own sentence ending in a dump of its built-in pillar list.
