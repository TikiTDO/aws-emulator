Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-deep-link-and-identity-center-refusals
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-deep-link-and-identity-center-refusals cloudwatchomni-view-definition-statement-and-name-rules cloudwatchomni-view-update-and-conflict-errors
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateOneTimeDeepLinkCode|4xx->4xx|$.message
Issue-Key: cloudwatchomni|CreateView|4xx->4xx|$.message
Issue-Key: cloudwatchomni|DeleteView|4xx->4xx|$.message
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# cloudwatchomni error messages diverge from AWS wording for validation, conflict and delete not-found

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateView

## Description
Four error responses return the right HTTP status and error type but a message AWS never emits: the range-validation message echoes the rejected value, the non-SELECT view definition leaks a SQL-parser diagnostic, and the view conflict / delete-not-found messages are re-worded. The message is part of the contract, so each is a defect. This bundle has FOUR causes; all four must be fixed and re-replayed. Two open issues cover neighbouring instances but not these replay keys: 20261109T172919Z-cloudwatchomni-validationexception-message-echoes-value-and-unordered-enum.md (echoed value / enum ordering on CreateDomain and CreateAccessGrant) and 20261111T022337Z-cloudwatchomni-conflict-and-validation-error-messages-diverge-from.md (CreateAccessProfile, CreateView, DeleteDomain, UpdateDomain from the static pass); also 20261109T172919Z-cloudwatchomni-resourcenotfoundexception-messages-do-not-match-aws-wording.md for Get-style not-found wording. None of them carries the steps below.

Cause 1 of 4 - CreateOneTimeDeepLinkCode range-validation message echoes the rejected value (fixture cloudwatchomni-deep-link-and-identity-center-refusals, step task#3).
AWS: Smithy CreateOneTimeDeepLinkCode input ttlSeconds is Integer with @range min 60, max 600; errors include ValidationException (httpError 400). Recorded behavior: HTTP 400 with message "1 validation error detected: Value at 'ttlSeconds' failed to satisfy constraint: Member must have value less than or equal to 600". Fixed text: "<N> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>"; the variable parts are the member path ('ttlSeconds') and the modeled bound (600). The rejected value is NOT included.
Emulator: returns 400 with "1 validation error detected: Value '601' at 'ttlSeconds' failed to satisfy constraint: Member must have value less than or equal to 600" - it interpolates the submitted value after 'Value'.
Why: a client that matches or logs the validation message sees text AWS does not produce, and for sensitive inputs the emulator echoes caller data AWS deliberately withholds.
Fix: the shared ValidationException message formatter used by the CreateOneTimeDeepLinkCode input validation - omit the offending value for range constraints.
Traces: cloudwatchomni-deep-link-and-identity-center-refusals#task#3.

Cause 2 of 4 - CreateView leaks a SQL-parser diagnostic instead of the modeled rule message (fixture cloudwatchomni-view-definition-statement-and-name-rules, step task#1; same cluster also at #task#2 and #task#3).
AWS: Smithy CreateView input definition is ViewDefinition (@length 1..10000) and the operation doc states 'A view is a named, reusable SQL query'; errors include ValidationException (httpError 400). Recorded behavior: a definition that is not a SELECT query is refused with HTTP 400 and the fixed message 'View definition must be a SELECT query' - no variable parts, and no detail about the submitted SQL.
Emulator: returns 400 with 'Invalid view definition: sql parser error: Expected: SELECT, VALUES, or a subquery in the query body, found: DROP at Line: 1, Column: 1' - the internal parser's error text, including the offending token and position.
Why: the message is implementation detail of the emulator's SQL parser and varies with the submitted statement, so a client asserting on or surfacing AWS's wording gets different text for every rejected definition, and the response advertises an internal component.
Fix: the CreateView handler's definition validation - classify any non-SELECT statement and emit the fixed AWS message rather than propagating the parser error.
Traces: cloudwatchomni-view-definition-statement-and-name-rules#task#1.

Cause 3 of 4 - CreateView duplicate-name conflict message re-worded (fixture cloudwatchomni-view-update-and-conflict-errors, step task#2).
AWS: Smithy CreateView errors include ConflictException (httpError 409). Recorded behavior: HTTP 409 with message 'View already exists: <viewName>' - fixed prefix 'View already exists: ' followed by the name taken from the request.
Emulator: returns 409 with "A view named '<viewName>' already exists." - different fixed text, quoted name, trailing period.
Why: clients that branch on or display the conflict message (idempotent-create wrappers commonly do) do not recognise AWS's text.
Fix: the CreateView handler's conflict path message.
Traces: cloudwatchomni-view-update-and-conflict-errors#task#2.

Cause 4 of 4 - DeleteView not-found message re-worded (fixture cloudwatchomni-view-update-and-conflict-errors, step task#9).
AWS: Smithy DeleteView errors are AccessDeniedException and ResourceNotFoundException (httpError 404). Recorded behavior: deleting a view that does not exist returns HTTP 404 with message 'View not found: <viewName>' - fixed prefix 'View not found: ' plus the requested name.
Emulator: returns 404 with "View '<viewName>' does not exist." - different fixed text and layout.
Why: same as Cause 3; a delete-if-exists wrapper that recognises AWS's not-found text fails to classify the error.
Fix: the DeleteView handler's ResourceNotFoundException message (shared view-not-found formatter with GetView/UpdateView).
Traces: cloudwatchomni-view-update-and-conflict-errors#task#9.

## Evidence
Cause 1 - expected 400 / got 400: $.message expected "1 validation error detected: Value at 'ttlSeconds' failed to satisfy constraint: Member must have value less than or equal to 600", got "1 validation error detected: Value '601' at 'ttlSeconds' failed to satisfy constraint: Member must have value less than or equal to 600".
Cause 2 - expected 400 / got 400: $.message expected 'View definition must be a SELECT query', got 'Invalid view definition: sql parser error: Expected: SELECT, VALUES, or a subquery in the query body, found: DROP at Line: 1, Column: 1' (cluster also covers #task#2 and #task#3 of the same fixture).
Cause 3 - expected 409 / got 409: $.message expected 'View already exists: view.sim2realprobecwo0060ca74', got "A view named 'view.sim2realprobecwo0060ca74' already exists." (the view name is the request value; the wording is the requirement).
Cause 4 - expected 404 / got 404: $.message expected 'View not found: view.sim2realprobecwo0060ca74', got "View 'view.sim2realprobecwo0060ca74' does not exist."

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect: cloudwatchomni-deep-link-and-identity-center-refusals#task#3 (CreateOneTimeDeepLinkCode with ttlSeconds above the modeled max of 600); cloudwatchomni-view-definition-statement-and-name-rules#task#1 (CreateView whose definition is not a SELECT statement); cloudwatchomni-view-update-and-conflict-errors#task#2 (CreateView re-using an existing view name); cloudwatchomni-view-update-and-conflict-errors#task#9 (DeleteView on an already-deleted view). Each returns the expected status with a non-AWS message.
