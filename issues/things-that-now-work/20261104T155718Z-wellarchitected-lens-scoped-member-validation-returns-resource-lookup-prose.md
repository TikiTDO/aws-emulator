Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-lens-review-answer-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-lens-review-answer-lifecycle wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|UpdateAnswer|4xx->4xx|$.Fields[*]
Issue-Key: wellarchitected|ListAnswers|4xx->4xx|$.Fields[*].Message
Issue-Key: wellarchitected|ListAnswers|4xx->4xx|$.Message
Issue-Related: 20261029T192731Z-wellarchitected-validationexception-envelopes-use-generic-framework-text-and-omit.md
Issue-Commit: f5f38caeaa03e56870175fb074275b9cd1e72c06

# Lens-scoped member validation returns resource-lookup prose instead of AWS field-validation envelope

> _Note: this issue was identified against the code base at commit `f5f38caeaa03e56870175fb074275b9cd1e72c06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAnswer

## Description
This bundle has 2 causes. Both are 400 ValidationException responses whose status is right but whose envelope is wrong in the same way: when a request names a lens-scoped member value the emulator cannot resolve inside the lens (a choice id under SelectedChoices, a PillarId), the emulator answers with its own resource-lookup sentence 'No <kind> with ID <id> was found in the lens with alias <alias>.' placed BOTH in ValidationException$Message (behind the '[Validation] ' prefix) and in Fields[0].Message, and it names the member it looked the id up by rather than the request member that failed validation. AWS instead reports these as field validation failures: a fixed generic Message with the member-specific detail carried in Fields[*], keyed by the input member name as modeled. Smithy: ValidationException{Message required, Reason: ValidationExceptionReason [UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER], Fields: list<ValidationExceptionField{Name, Message}>}, httpError 400; UpdateAnswer PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId} with SelectedChoices: list; ListAnswers GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers with PillarId @httpQuery @length 1..64.

This is NOT the generic-renderer bundle already open as 'ValidationException envelopes use generic framework text and omit Reason and Fields' (20261029T192731Z-...-validationexception-envelopes-...): there the emulator emits the shared '1 validation error detected: ...' framework string; here it emits bespoke lens-lookup prose, from the lens-resolution path of these two handlers. It is also not the open 'Built-in lens catalog content and ARN form diverge from the AWS lens library' issue (20261029T192731Z-...-built-in-lens-catalog-content-and-arn-form.md): that issue owns the catalog's question/pillar/choice identifiers, while the two causes below are about the error envelope that must be produced once a value does not resolve, whatever the catalog contains. Neither cluster is marked owned by any open issue in the replay report.

Cause 1 of 2: UpdateAnswer rejects an unresolvable selected choice with a QuestionId lookup message instead of the SelectedChoices field-validation envelope.
AWS: when SelectedChoices contains choice ids that are not valid for the addressed question, the request is refused 400 ValidationException with the FIXED Message '[Validation] The validation of one or more fields has failed.' and exactly one Fields entry whose Name is the offending input member, 'SelectedChoices', and whose Message is 'SelectedChoices contains invalid choices. Valid choices are: [<choiceIds>].' — fixed text apart from <choiceIds>, the comma-separated list of the choice ids the addressed question actually defines, taken from the lens the request names. The detail belongs in Fields[*].Message; Message itself carries no value from the request.
Emulator: Message '[Validation] No question with ID <questionId> was found in the lens with alias <lensAlias>.' and Fields[0] = {Name: 'QuestionId', Message: 'No question with ID <questionId> was found in the lens with alias <lensAlias>.'}.
Why: a client that attributes a validation failure to an input member reads Name 'QuestionId' and blames the path label it sent correctly, instead of the SelectedChoices body member it must fix; and the one piece of actionable data AWS supplies — the set of valid choice ids for that question — is absent, so a client cannot repair the request or show the operator the allowed values. Clients matching on AWS's fixed Message text also miss the error class entirely, because the emulator's Message varies with the request.
Fix: the UpdateAnswer handler's answer/choice validation path, which must validate SelectedChoices against the addressed question's choice set and build the field-validation envelope there, rather than returning its lens/question resolution error as the validation result.
Traces: wellarchitected-lens-review-answer-lifecycle#task#9.

Cause 2 of 2: ListAnswers rejects an unresolvable PillarId with the same resource-lookup prose, which does not conform to the recorded envelope shape.
AWS: recorded behavior only — the fixture grades the shape of $.Message and $.Fields[0].Message for this step rather than their literal text (they are recorded as '<shaped-not-asserted>'), so the recording states that a 400 ValidationException with both members present is required and that the emulator's text does not conform to the shape AWS produced. Hypothesis, grounded in the sibling recording in cause 1: AWS uses the same field-validation envelope here — the fixed Message '[Validation] The validation of one or more fields has failed.' with the pillar-specific detail in a Fields entry keyed by the input member 'PillarId' — and not a resource-lookup sentence.
Emulator: Message '[Validation] No pillar with ID <pillarId> was found in the lens with alias <lensAlias>.' and Fields[0].Message 'No pillar with ID <pillarId> was found in the lens with alias <lensAlias>.' (the same sentence, unprefixed).
Why: an unknown PillarId on a list call is a filter-value validation failure; a client reading the envelope gets the emulator's own prose in both members, so neither message matching nor member attribution behaves as it does against AWS.
Fix: the ListAnswers handler's PillarId filter validation, which should emit the shared field-validation envelope for an out-of-lens pillar instead of propagating its lens-catalog lookup error text.
Traces: wellarchitected-pagination-and-limit-validation#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 94 steps, 66 passed, 28 mismatched, score 70.2%). Both clusters below report service=wellarchitected, verdict=mismatch, expected 400 / actual 400, and an empty stub_path and empty owned_by.

Cause 1 — wellarchitected-lens-review-answer-lifecycle#task#9, UpdateAnswer, divergence_key 'service=wellarchitected|UpdateAnswer|status=400->400|keys=$.Fields[0],$.Message':
  $.Fields[0]: expected {'Name': 'SelectedChoices', 'Message': 'SelectedChoices contains invalid choices. Valid choices are: [cost_cloud_financial_management_function, cost_cloud_financial_management_partnership, cost_cloud_financial_management_budget_forecast, cost_cloud_financial_management_cost_awareness, cost_cloud_financial_management_proactive_process, cost_cloud_financial_management_scheduled, cost_cloud_financial_management_quantify_value, cost_cloud_financial_management_usage_report, cost_cloud_financial_management_culture, cost_cloud_financial_management_no].'}, got {'Name': 'QuestionId', 'Message': 'No question with ID cloud-financial-management was found in the lens with alias wellarchitected.'} (reported both as missing and as extra).
  $.Message: expected '[Validation] The validation of one or more fields has failed.', got '[Validation] No question with ID cloud-financial-management was found in the lens with alias wellarchitected.'.
  Fixed vs variable: in the expected envelope, Message is entirely fixed text; in Fields[0], Name ('SelectedChoices') and the sentence 'SelectedChoices contains invalid choices. Valid choices are: [...]' are fixed, and only the bracketed choice-id list is a value derived from the lens/question the request addresses. The recorded ids above are that one run's values and are not part of the requirement.

Cause 2 — wellarchitected-pagination-and-limit-validation#task#6, ListAnswers, divergence_key 'service=wellarchitected|ListAnswers|status=400->400|keys=$.Fields[0].Message,$.Message':
  $.Fields[0].Message: expected '<shaped-not-asserted>', got 'No pillar with ID sim2realprobe was found in the lens with alias wellarchitected.'.
  $.Message: expected '<shaped-not-asserted>', got '[Validation] No pillar with ID sim2realprobe was found in the lens with alias wellarchitected.'.
  The expected values are shape-graded, so the requirement is the envelope shape AWS emitted, not these literals; the pillar id 'sim2realprobe' and the lens alias 'wellarchitected' are request values filling the emulator's template.

Smithy (services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/...): UpdateAnswer input {WorkloadId @httpLabel required, LensAlias @httpLabel required, QuestionId @httpLabel required, SelectedChoices list, ChoiceUpdates map, Notes, IsApplicable, Reason: AnswerReason}, errors include ValidationException (client, httpError 400); ListAnswers input {WorkloadId @httpLabel required, LensAlias @httpLabel required, PillarId @httpQuery @length 1..64, MilestoneNumber, NextToken, MaxResults, QuestionPriority}, errors include ValidationException (client, httpError 400); ValidationException{Message required, Reason enum, Fields list<ValidationExceptionField{Name, Message}>}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: create a workload with the wellarchitected lens, then PATCH /workloads/{WorkloadId}/lensReviews/wellarchitected/answers/{QuestionId} with SelectedChoices containing one or more choice ids that the addressed question does not define. Read $.Message and $.Fields[0]: AWS returns Message '[Validation] The validation of one or more fields has failed.' and Fields[0].Name 'SelectedChoices' with the valid-choice list; the emulator returns a 'No question with ID ... was found in the lens with alias ...' sentence with Fields[0].Name 'QuestionId'. Replay wellarchitected-lens-review-answer-lifecycle#task#9 to confirm.
4. Cause 2: GET /workloads/{WorkloadId}/lensReviews/wellarchitected/answers?PillarId=<a pillar id the lens does not define> and read $.Message and $.Fields[0].Message: the emulator returns its 'No pillar with ID ... was found in the lens with alias ...' sentence in both, which does not conform to the recorded envelope. Replay wellarchitected-pagination-and-limit-validation#task#6 to confirm.
5. Both steps must pass replay before closing; they are separate causes in separate fixtures.
