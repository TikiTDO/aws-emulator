Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-recommended-actions-error-invalid-filter-key
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-recommended-actions-error-invalid-filter-key bcm-recommended-actions-error-invalid-match-option bcm-recommended-actions-error-malformed-next-token
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.fieldList[name="filter.actions.1.member.key"].message
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.message
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.fieldList[name="filter.actions.1.member.matchOption"].message
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# ListRecommendedActions error messages diverge: enum value set order, echoed value, nextToken wording

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
ListRecommendedActions answers every one of the corpus's three error-path fixtures with the right HTTP status (400) and the right modelled error shape (ValidationException, with reason and fieldList present), but the human-readable `message` text — which is part of the error contract — is worded differently from AWS in all three. Three distinct causes, all in this operation's refusal rendering; they are bundled because they share one service and one operation.

Cause 1 of 3: the enum-value-set clause for ActionFilter$key is rendered in a different order than AWS uses, and the summary message echoes the rejected value.
  AWS: Smithy: ListRecommendedActions.filter -> RequestFilter$actions -> ActionFilter$key targets the enum com.amazonaws.bcmrecommendedactions#FilterName (members FEATURE, SEVERITY, TYPE), @required; an unmodelled value is refused with ValidationException (errorKind client, httpError 400). The model does not state the message text, so the wording below is recorded behavior only. The recording shows, for a single refused enum member, the top-level message `1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]` and a fieldList entry whose name is that member path and whose message is `Value <rejected value> ... failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]` (the fixture redacts the request-supplied span as `[redacted]`). Fixed text of this message class: the `N validation error(s) detected: ` envelope, the literal `Value `/` at `/` failed to satisfy constraint: Member must satisfy enum value set: [`...`]` frame, and the enum value listing itself in the order AWS emits it for this member — FEATURE, TYPE, SEVERITY. Variable parts taken from the request: the error count, the dotted member path (here the list-element path for the first ActionFilter's `key`), and, in the fieldList entry only, the rejected value. Hypothesis: AWS does not echo the rejected value in the top-level summary message for this member (the recording's summary reads `Value at '<path>'` and was NOT redacted there, while the fieldList entry WAS), and echoes it only in the per-field message.
  Emulator: emits the same frame but lists the enum set in the model's declaration/alphabetical order — `[FEATURE, SEVERITY, TYPE]` — in both the summary message and the fieldList entry, and additionally echoes the rejected value in the summary message (`1 validation error detected: Value '<rejected value>' at '<path>' failed to satisfy constraint: ...`).
  Why: a client that matches or displays the validation message (and any test or SDK-level assertion pinned to AWS's wording) sees a different permitted-value listing and a different summary sentence than AWS returns, so error-text assertions fail and operators comparing emulator output to AWS see a spurious difference in which values the service claims to accept.
  Fix: the operation's refusal rendering — `field_validation_failed` in the ListRecommendedActions handler and the framework violation clause it interpolates (the `constraints::envelope` summary and `Violation::message()` per-field clause) reached through the ModelledValidation seam. The enum listing must be emitted in AWS's order for the member rather than the schema's, and the summary clause must not interpolate the rejected value.
  Traces: bcm-recommended-actions-error-invalid-filter-key#task#1.

Cause 2 of 3: the same two defects for ActionFilter$matchOption, whose AWS listing order also differs from the model's.
  AWS: Smithy: ActionFilter$matchOption targets com.amazonaws.bcmrecommendedactions#MatchOption (members EQUALS, NOT_EQUALS), @required; refused with ValidationException (400). Message text is recorded behavior only: the summary reads `1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]`, with the per-field entry carrying the same clause plus the rejected value (redacted in the fixture). So the required fixed text lists this member's set as NOT_EQUALS, EQUALS — i.e. the order is a per-member property of AWS's message, NOT the Smithy declaration order, which is why it must be carried explicitly and cannot be derived by sorting.
  Emulator: lists `[EQUALS, NOT_EQUALS]` and again echoes the rejected value in the summary message.
  Why: same client impact as cause 1, on a second member — and it shows a fix that merely sorts or reuses model order cannot satisfy both members.
  Fix: same component as cause 1; the per-member value listing used in the enum clause.
  Traces: bcm-recommended-actions-error-invalid-match-option#task#1.

Cause 3 of 3: a malformed/unmintable nextToken is refused with the emulator's own sentence instead of AWS's pagination-token message.
  AWS: Smithy: ListRecommendedActions is @paginated (inputToken/outputToken nextToken); NextToken is a string with @length 1..8192 and pattern ^[\\S\\s]*$, so a token that satisfies those constraints but is not a token the service minted is refused by the handler, not by constraint validation, with ValidationException (400). Message text is recorded behavior only: `Pagination token exception in operation '<OperationName>': invalid payload wrapping schema <n>`. Fixed text: the `Pagination token exception in operation '` prefix, the `': invalid payload wrapping schema ` separator. Variable parts: the operation name from the request's routing key, and a small integer derived from the supplied token's decoded payload (the recorded run shows -24949 for its token; that number is a value of that run's request, not part of the template, so it must be computed from the token and never hard-coded).
  Emulator: answers `The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged.` (status 400 and the error shape are otherwise correct; only $.message diverges).
  Why: the two sentences are not interchangeable — a client or operator keying on AWS's pagination-token message class (or parsing the operation name out of it) does not recognize the emulator's wording, and a caller cannot tell from it that the token failed payload decoding.
  Fix: the ListRecommendedActions handler's token parsing — the `malformed` refusal closure in `parse_token` (see Traces) must word its message the way AWS words a pagination-token decode failure, naming the operation and the decoded-payload discriminator.
  Traces: bcm-recommended-actions-error-malformed-next-token#task#1; code: services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs:570-579 (parse_token/malformed), and src/service.rs:289-306 plus the same file's field_validation_failed at ops/list_recommended_actions.rs:219-239 for causes 1 and 2.

Neither open issue on this service covers these: '20261028T144507Z-bcm-recommended-actions-listrecommendedactions-maintainability' is about the documented unrouted-operation default vs the crate's 501 test, and '20261029T134218Z-bcm-recommended-actions-listrecommendedactions-wrong-behavior--r4' is about the boot inventory's lastUpdatedTimeStamp. Three of the corpus's six steps for this service pass; all three failures are message-text only.

## Evidence
Replay of trace set simparity-wire-aws-v1: 6 steps, 3 passed, 3 mismatched, score 50%. Emulator up (/__admin/health 200). All three mismatches are service=bcm-recommended-actions, operation=ListRecommendedActions, expected_status=400 / actual_status=400 — status and error shape match, only message text diverges.

Cause 1 — fixture bcm-recommended-actions-error-invalid-filter-key, step task#1:
  $.message expected: "1 validation error detected: Value at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]"
  $.message actual:   "1 validation error detected: Value 'NOT_A_REAL_FILTER_KEY' at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, SEVERITY, TYPE]"
  $.fieldList[name="filter.actions.1.member.key"].message expected: "Value [redacted] failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]"
  actual: "Value 'NOT_A_REAL_FILTER_KEY' at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, SEVERITY, TYPE]"
  (Fixed text vs values: the envelope, the constraint frame and the enum listing are fixed; the error count, member path and — in the fieldList entry — the rejected value come from the request. 'NOT_A_REAL_FILTER_KEY' and 'filter.actions.1.member.key' are this recording's request values, not requirements.)

Cause 2 — fixture bcm-recommended-actions-error-invalid-match-option, step task#1:
  $.message expected: "1 validation error detected: Value at 'filter.actions.1.member.matchOption' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]"
  $.message actual:   "1 validation error detected: Value 'NOT_A_REAL_MATCH_OPTION' at 'filter.actions.1.member.matchOption' failed to satisfy constraint: Member must satisfy enum value set: [EQUALS, NOT_EQUALS]"
  $.fieldList[...matchOption].message expected: "Value [redacted] failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]" vs actual listing "[EQUALS, NOT_EQUALS]".
  Model cross-check (smithy_query shape MatchOption): values EQUALS, NOT_EQUALS; FilterName: FEATURE, SEVERITY, TYPE — i.e. the emulator reproduces model order while AWS's recorded listings are [NOT_EQUALS, EQUALS] and [FEATURE, TYPE, SEVERITY].

Cause 3 — fixture bcm-recommended-actions-error-malformed-next-token, step task#1:
  $.message expected: "Pagination token exception in operation 'ListRecommendedActions': invalid payload wrapping schema -24949"
  $.message actual:   "The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged."
  (Fixed text: "Pagination token exception in operation '" ... "': invalid payload wrapping schema "; variables: the operation name and an integer derived from the supplied token. -24949 is that run's derived value.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST an awsJson ListRecommendedActions request whose filter.actions[0].key is a string outside the FilterName enum (fixture bcm-recommended-actions-error-invalid-filter-key, step task#1). Compare $.message and $.fieldList[0].message against the recorded 400 body: the enum listing order differs and the summary message carries the rejected value.
3. Cause 2: same request with filter.actions[0].matchOption outside the MatchOption enum (fixture bcm-recommended-actions-error-invalid-match-option, step task#1); the listing is [EQUALS, NOT_EQUALS] instead of AWS's [NOT_EQUALS, EQUALS].
4. Cause 3: call ListRecommendedActions with a nextToken that satisfies NextToken's length/pattern constraints but was not minted by the service (fixture bcm-recommended-actions-error-malformed-next-token, step task#1); the 400 ValidationException message is the emulator's own sentence instead of AWS's "Pagination token exception in operation '...': invalid payload wrapping schema <n>".
