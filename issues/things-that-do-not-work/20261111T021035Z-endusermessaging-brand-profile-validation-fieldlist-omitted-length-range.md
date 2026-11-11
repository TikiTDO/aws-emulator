Issue-Source: trace-sweep-divergence
Issue-Trace: endusermessaging-brand-profile-attribute-create-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: endusermessaging-brand-profile-attribute-create-validation endusermessaging-brand-profile-identifier-and-list-errors
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging|CreateBrandProfileAttributes|4xx->4xx|$.fieldList
Issue-Key: endusermessaging|CreateBrandProfileAttributes|4xx->4xx|$.message
Issue-Key: endusermessaging|UpdateBrandProfile|4xx->2xx|$.message
Issue-Key: endusermessaging|UpdateBrandProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: endusermessaging|UpdateBrandProfile|4xx->2xx|status
Issue-Key: endusermessaging|ListBrandProfiles|4xx->4xx|$.fieldList
Issue-Key: endusermessaging|ListBrandProfiles|4xx->4xx|$.message
Issue-Commit: deeb1944d1d0db8b9764b546c3d5c87bdcf95957

# endusermessaging brand-profile validation: fieldList omitted, length/range sentences re-worded, empty UpdateBrandProfile accepted

> _Note: this issue was identified against the code base at commit `deeb1944d1d0db8b9764b546c3d5c87bdcf95957`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrandProfileAttributes

## Description
THREE root causes in how endusermessaging refuses bad brand-profile input, all observed replaying the wire corpus simparity-wire-aws-v1 against the running (bootstrapped) emulator. The replay report shows all three steps unowned by any open issue. The open issue '20261104T183251Z-endusermessaging-input-validation-refusals-omit-fieldlist-and-diverge.md' describes the same validation-envelope family for OTHER operations (CreateBrandProfileFromRegistration @pattern, CreateNotifyCodeConfiguration @range, GetBrandProfile id-format, CreateBrandProfile idempotency) and its replay key covers only those four steps; it carries no key for CreateBrandProfileAttributes, ListBrandProfiles or the UpdateBrandProfile empty-update step, so none of the three causes below is exercised by its replay. Note also that CreateBrandProfileAttributes is no longer an unimplemented stub (the replay reports no stub for it), so the old stub issue does not cover cause 1 either.

=== Cause 1 of 3: list @length violation on CreateBrandProfileAttributes omits fieldList and re-words AWS's length sentence ===
AWS: Smithy: CreateBrandProfileAttributes.attributes targets BrandProfileAttributeInputList with @required and @length min 1 max 10; the operation declares ValidationException (HTTP 400, @error client; members message @required and fieldList -> ValidationExceptionFieldList of ValidationExceptionField{path @required 'A JSONPointer expression to the structure member whose value failed to satisfy the modeled constraints', message @required}). For a list that violates a two-sided @length AWS answers 400 ValidationException whose message is '<N> validation error detected. Value with length <actualLength> at '<json-pointer>' failed to satisfy constraint: Member must have length between <min> and <max>, inclusive' and populates fieldList with one entry per failure: path is the member's JSON pointer ('/attributes') and message is that same per-field sentence. Fixed text: '<N> validation error detected. Value with length ', "' failed to satisfy constraint: Member must have length between ", ' and ', ', inclusive'; the variable parts are the failure count, the submitted collection's length, the member's JSON pointer and the @length bounds taken from the model (1 and 10 here). The submitted VALUES are not echoed.
Emulator: answers 400 (correct status) with message "1 validation error detected: Value '<n> items' at 'attributes' failed to satisfy constraint: Member must have length less than or equal to 10" and no fieldList at all — a colon instead of a period after 'detected', a '<n> items' rendering where AWS writes the bare length, a bare dotted member name instead of a JSON pointer, and a one-sided 'less than or equal to <max>' sentence where AWS uses the two-sided inclusive 'between <min> and <max>' form for a @length with both bounds.
Why: a client or console that reads fieldList to point a user at the offending member gets nothing and must parse prose; a client matching AWS's envelope (period, 'Value with length <len>', '/attributes' pointer, inclusive two-sided sentence) never matches, so the refusal is reported to the user as an unknown validation failure.
Fix: the service's shared input-constraint validation / ValidationException renderer, as reached from CreateBrandProfileAttributes' validation — render AWS's length template and emit a ValidationExceptionField per failed constraint with a JSON-pointer path.
Traces: endusermessaging-brand-profile-attribute-create-validation#task#2 (cluster also covers #task#9).

=== Cause 2 of 3: UpdateBrandProfile with no updatable member is served 200 instead of the 400 'at least one field' refusal ===
AWS: Smithy: UpdateBrandProfile input is brandProfileId (@httpLabel, @required) plus the two OPTIONAL updatable members brandProfileName and deletionProtectionEnabled; the operation declares ValidationException (HTTP 400). A request that carries neither updatable member is refused before any state change with HTTP 400 and the message 'At least one field must be provided for update' (recorded behavior: the recording asserts status 400, that exact message, and the presence of x-amzn-errortype, whose value is not asserted). The message is entirely fixed text — no request or state values appear in it.
Emulator: answers 200 with the unchanged profile's body, no message and no x-amzn-errortype header, and it additionally bumps updatedAt — so a no-op request is reported as a successful update.
Why: a client that sends an update with all mutable fields empty (a common result of building the request from a partially-filled form or a diff that turned out empty) is told the update succeeded, and the resource's updatedAt moves, instead of being told the request was malformed. Nothing distinguishes 'nothing to change' from 'changes applied', so a caller cannot detect that its intended change was dropped.
Fix: UpdateBrandProfile's validation, ahead of the store mutation — if neither brandProfileName nor deletionProtectionEnabled is present, raise ValidationException with the fixed 'At least one field must be provided for update' message and leave the stored record (including updatedAt) untouched.
Traces: endusermessaging-brand-profile-identifier-and-list-errors#task#4; the missing gate is visible at services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/update_brand_profile.rs:195-203, which applies only the members present and then unconditionally sets updated_at.

=== Cause 3 of 3: ListBrandProfiles maxResults @range violation omits fieldList and uses the one-sided constraint sentence ===
AWS: Smithy: ListBrandProfiles.maxResults is an @httpQuery('maxResults') integer with @range min 1 max 100; the operation declares ValidationException (HTTP 400). For a value outside that range AWS answers 400 ValidationException with message '<N> validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive' and fieldList [{path: '/maxResults', message: <that same sentence>}]. Fixed text: '<N> validation error detected. Value at ', "' failed to satisfy constraint: Member must be between ", ' and ', ', inclusive'; the variable parts are the failure count, the member's JSON pointer and the @range bounds from the model. The rejected value is NOT echoed.
Emulator: answers 400 with message "1 validation error detected: Value '<submitted>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" and no fieldList — colon instead of period, the submitted value interpolated, a dotted member name instead of a JSON pointer, and a one-sided sentence where the model's @range has both bounds.
Why: a client that shows the valid page-size range to the caller, or branches on the constraint sentence, gets neither the documented 1..100 inclusive wording nor a machine-readable field entry, and a paginator that retries with a corrected maxResults cannot learn the bound from the error.
Fix: the same shared validation / ValidationException renderer as reached from ListBrandProfiles' query-parameter validation — render the two-sided inclusive range sentence and populate fieldList with the member's JSON pointer.
Traces: endusermessaging-brand-profile-identifier-and-list-errors#task#5.

## Evidence
Cause 1 — fixture endusermessaging-brand-profile-attribute-create-validation step task#2 (CreateBrandProfileAttributes, 400 vs 400; cluster also covers task#9):
  $.message expected: "1 validation error detected. Value with length 11 at '/attributes' failed to satisfy constraint: Member must have length between 1 and 10, inclusive"
  $.message actual:   "1 validation error detected: Value '11 items' at 'attributes' failed to satisfy constraint: Member must have length less than or equal to 10"
  $.fieldList expected: [{'path': '/attributes', 'message': "Value with length 11 at '/attributes' failed to satisfy constraint: Member must have length between 1 and 10, inclusive"}]; actual: missing (None).
  Fixed vs variable: 'N validation error detected. Value with length ' / "' failed to satisfy constraint: Member must have length between " / ', inclusive' are fixed; '/attributes' is the member's JSON pointer and 1/10 are the model's @length bounds. The length 11 is this recording's submitted list size.

Cause 2 — fixture endusermessaging-brand-profile-identifier-and-list-errors step task#4 (UpdateBrandProfile):
  status expected 400, got 200.
  $.message expected: 'At least one field must be provided for update' (entirely fixed text); actual: absent (None).
  $headers.x-amzn-errortype expected present (value not asserted); actual: absent.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/update_brand_profile.rs:195-203 — the handler applies only the members present and then sets profile.updated_at unconditionally, with no check that at least one updatable member was supplied.

Cause 3 — fixture endusermessaging-brand-profile-identifier-and-list-errors step task#5 (ListBrandProfiles, 400 vs 400):
  $.message expected: "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive"
  $.message actual:   "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  $.fieldList expected: [{'path': '/maxResults', 'message': "Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive"}]; actual: missing (None).
  1 and 100 are the model's @range bounds on MaxResults (not recording-specific); '101' is this request's submitted value, which AWS does not echo.

## Steps to Reproduce
Cause 1:
1. POST /v1/brand-profiles with a valid name to create a profile (200).
2. POST /v1/brand-profiles/{brandProfileId}/attributes with an 'attributes' list holding MORE than 10 entries (e.g. 11).
3. The emulator answers 400 with message "1 validation error detected: Value '11 items' at 'attributes' failed to satisfy constraint: Member must have length less than or equal to 10" and no fieldList. AWS answers 400 with "1 validation error detected. Value with length 11 at '/attributes' failed to satisfy constraint: Member must have length between 1 and 10, inclusive" plus the matching fieldList entry at path '/attributes'.

Cause 2:
1. Create a brand profile (200) and note its brandProfileId.
2. PUT /v1/brand-profiles/{brandProfileId} with a body carrying NEITHER brandProfileName NOR deletionProtectionEnabled (e.g. {}).
3. The emulator answers 200 with the unchanged profile (and a bumped updatedAt); AWS answers 400 with message 'At least one field must be provided for update' and an x-amzn-errortype header, leaving the record untouched.

Cause 3:
1. GET /v1/brand-profiles?maxResults=101 (any value outside 1..100).
2. The emulator answers 400 with 'Member must have value less than or equal to 100', the value echoed, a dotted path and no fieldList; AWS answers 400 with "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 100, inclusive" and fieldList [{path: '/maxResults', ...}].

Replay: wire corpus simparity-wire-aws-v1, fixtures endusermessaging-brand-profile-attribute-create-validation#task#2, endusermessaging-brand-profile-identifier-and-list-errors#task#4, endusermessaging-brand-profile-identifier-and-list-errors#task#5.
