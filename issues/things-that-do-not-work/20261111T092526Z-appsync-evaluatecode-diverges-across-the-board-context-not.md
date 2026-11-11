Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-eval-js-error-surface
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-eval-js-error-surface appsync-eval-js-transformations appsync-unit-resolver-code-evaluation
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.error.message
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.logs
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.outErrors
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.stash
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.error
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.evaluationResult
Issue-Key: appsync|EvaluateCode|4xx->2xx|$.message
Issue-Key: appsync|EvaluateCode|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|EvaluateCode|4xx->2xx|status
Issue-Key: appsync|EvaluateCode|4xx->4xx|$.message
Issue-Key: appsync|EvaluateCode|2xx->2xx|$.logs[*]
Issue-Key: appsync|EvaluateCode|2xx->4xx|$.error
Issue-Key: appsync|EvaluateCode|2xx->4xx|status
Issue-Commit: 323587cc72041ca5bad5e7b036e306f86c24b041

# AppSync EvaluateCode diverges across the board: context not bound, no code diagnostics, response envelope incomplete (10 causes)

> _Note: this issue was identified against the code base at commit `323587cc72041ca5bad5e7b036e306f86c24b041`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EvaluateCode

## Description
The emulator's EvaluateCode (POST /v1/dataplane-evaluatecode) diverges from AWS in three connected ways that the wire corpus exercises ten times: (a) the APPSYNC_JS evaluation context is only partially bound, so ordinary handler code dereferences `undefined` and the evaluation fails where AWS evaluates it; (b) the response envelope is incomplete — `stash` and `outErrors` are never emitted, and `logs` is always an empty list; (c) code diagnostics and input refusals are wrong — lint errors are not detected at all, a parse error becomes an HTTP 400 refusal instead of a 200 carrying codeErrors, a missing `function` member is accepted, and the bad-runtimeVersion refusal is emulator-authored text.

Shared Smithy basis for all ten causes — Smithy: EvaluateCode, POST /v1/dataplane-evaluatecode, http code 200; input runtime (AppSyncRuntime, required), code (Code, required, @length 1..32768), context (Context, required, @length 2..28000), function (String, optional); output EvaluateCodeResponse{evaluationResult (EvaluationResult, @length 0..65536), error (EvaluateCodeErrorDetail{message, codeErrors: list<CodeError{errorType, value, location: CodeErrorLocation{line, column, span}}>}), logs (Logs, list<String>), stash (Stash, string), outErrors (OutErrors, string)}; modeled errors AccessDeniedException(403), BadRequestException(400), InternalFailureException(500). CodeError$errorType is documented with the examples LINT_ERROR and PARSER_ERROR; CodeErrorLocation carries line/column/span.

NOTE: open behavioral issue 20261110T215409Z-appsync-10-net-new-causes-graphql-realtime-endpoints.md describes the same EvaluateCode sandbox family (missing ctx.args alias, missing `util` global, compile failures raising BadRequestException) from the behavioral suite; the replay did not mark any of the ten wire clusters below as owned by it, so they are filed here and may well be closed by the same fix. Open issue 20261107T183746Z-appsync-authorization-gaps-domain-name-and-flushapicache-calls.md covers only EvaluateCode authorization, not its result fidelity.

=== Cause 1 of 10: the evaluation context is not fully bound, so a handler that reads a context member raises instead of running, and stash/outErrors/logs are wrong (fixture appsync-eval-js-error-surface, step task#1; same cluster also hits task#2) ===
AWS: recorded behavior only. The JSON document submitted in EvaluateCode$context is bound into the handler's `ctx` so that every member the recording's handler reads is reachable, and a handler that validates its input and calls util.error("<message>") returns HTTP 200 with error.message equal to the handler's own message — the fixed text is whatever string the code passed to util.error, i.e. a value from the request's `code`, not service text. In the same response AWS omits `logs` entirely when the handler logged nothing, and returns stash as a JSON-object string ("{}" when the handler set no stash keys) and outErrors as a JSON-array string ("[]" when util.appendError was never called).
Emulator: error.message is a sandbox diagnostic of the form "cannot read property '<name>' of undefined" — the handler never reached its own util.error call, because the context member it dereferenced is unbound (Hypothesis: only `arguments` is bound, so the documented alias ctx.args — and anything else the recording's handlers read — is undefined). In the same response `logs` is present as [] where AWS omits it, and `stash` and `outErrors` are absent entirely.
Why: a client using EvaluateCode as AppSync documents it — to test a resolver's JS before CreateResolver — is told its own correct code is broken, with a sandbox message instead of the validation message the code raises. Reading response['stash'] / response['outErrors'] (always present on AWS) raises KeyError/None.
Fix: the EvaluateCode handler's APPSYNC_JS context binding, plus its response projection/serializer for logs/stash/outErrors.
Traces: appsync-eval-js-error-surface#task#1 (representative), appsync-eval-js-error-surface#task#2.

=== Cause 2 of 10: lint diagnostics are never produced — unsupported JS constructs evaluate as if they were legal (fixture appsync-eval-js-error-surface, step task#3) ===
AWS: Smithy CodeError$errorType documents LINT_ERROR; recorded behavior: code using constructs APPSYNC_JS forbids is answered with HTTP 200, evaluationResult ABSENT and error populated: error.codeErrors holds one entry per diagnostic with errorType "LINT_ERROR", value "code.js(<line>,<column>): error @aws-appsync/<rule>: <rule text>" and location{line, column, span} pointing at the offending source position, and error.message is the concatenation of those entries' texts. The recording shows the rules @aws-appsync/no-for ("For statements are not supported.") and @aws-appsync/no-disallowed-unary-operators ("The operator ++ is not supported"). The rule names and texts are fixed; the file name, line, column and span are derived from the submitted `code`.
Emulator: HTTP 200 with evaluationResult "{}" and NO error member at all — the for-statement and ++ operator are simply executed, so no LINT_ERROR is ever reported. `logs` is again [] where AWS omits it.
Why: EvaluateCode is the documented pre-flight lint for APPSYNC_JS resolvers. Code the emulator reports as clean is rejected by real AppSync at CreateResolver/UpdateResolver time, which is precisely the failure the operation exists to prevent.
Fix: the EvaluateCode handler's code-validation stage (APPSYNC_JS lint rules) and the serialization of its diagnostics into EvaluateCodeResponse$error.codeErrors.
Traces: appsync-eval-js-error-surface#task#3.

=== Cause 3 of 10: a request with no `function` member is accepted instead of refused (fixture appsync-eval-js-error-surface, step task#4) ===
AWS: Smithy marks EvaluateCode$function optional, so this is recorded behavior only: when `function` is omitted the request is refused with HTTP 400 and the message "Function must be provided" — entirely fixed text, with no value interpolated from the request.
Emulator: HTTP 200 with an evaluation result; no $.message and no x-amzn-errortype header.
Why: the client gets a result for a request AWS never evaluates, so the handler it believes it tested (request vs response) was chosen by the emulator rather than by the caller; the refusal its error handling is written against never arrives.
Fix: the EvaluateCode handler's input validation (require `function` before evaluating) and its BadRequestException path.
Traces: appsync-eval-js-error-surface#task#4.

=== Cause 4 of 10: the invalid-runtimeVersion refusal message is emulator-authored (fixture appsync-eval-js-error-surface, step task#5) ===
AWS: recorded behavior only (AppSyncRuntime$runtimeVersion is an unconstrained string in the model). An unsupported runtimeVersion is refused with HTTP 400 and the message "Runtime version invalid." — fixed text, no request value interpolated.
Emulator: HTTP 400 with the message "Invalid runtimeVersion given.".
Why: the status is right but the text is not AWS's; clients and test suites that match on the message (and humans reading logs) see wording no AWS region produces.
Fix: the EvaluateCode handler's runtime validation message text.
Traces: appsync-eval-js-error-surface#task#5.

=== Cause 5 of 10: a handler AWS evaluates successfully fails with a sandbox error and no result (fixture appsync-eval-js-error-surface, step task#6) ===
AWS: recorded behavior only. The same handler returns HTTP 200 with evaluationResult set to the JSON string it computed from the submitted context, error ABSENT, stash "{}" and outErrors "[]".
Emulator: evaluationResult is absent and error is present with message "cannot read property '<name>' of undefined"; stash and outErrors are absent. Same unbound-context and same envelope gap as Cause 1, on a positive-path evaluation.
Why: the operation returns no result at all for valid code, so a client cannot use it to verify a resolver's output shape.
Fix: as Cause 1 — the EvaluateCode handler's context binding and response projection.
Traces: appsync-eval-js-error-surface#task#6.

=== Cause 6 of 10: a multi-stage transformation handler errors out, and its console.* output is not captured in `logs` (fixture appsync-eval-js-transformations, step task#1) ===
AWS: recorded behavior only. The handler runs to completion: evaluationResult is the DynamoDB-shaped request document it builds from the context, error is absent, stash is the JSON string of the keys the handler wrote into ctx.stash, outErrors is "[]", and `logs` holds one entry per console call, in call order, formatted "<LEVEL> - code.js:<line>:<column>: <arguments rendered as JSON, space-separated>" (LEVEL is INFO for console.log and ERROR for console.error; the file name is code.js and line/column are the call's position in the submitted `code`).
Emulator: error = {message: "cannot read property '<name>' of undefined"}, evaluationResult absent, stash and outErrors absent, and `logs` is [] — no console output is captured even for the statements that did run.
Why: logs are how a resolver author debugs a handler; an always-empty logs list plus a missing result makes the evaluator useless for the purpose it is documented for.
Fix: the EvaluateCode handler's context binding, its console capture into EvaluateCodeResponse$logs, and the stash/outErrors projection.
Traces: appsync-eval-js-transformations#task#1.

=== Cause 7 of 10: appended errors are not returned in outErrors (fixture appsync-eval-js-transformations, step task#2) ===
AWS: recorded behavior only. outErrors is a JSON-array string holding one object per util.appendError call, each with the members message/type/data/info taken from that call's arguments (absent arguments serialize as null); evaluationResult is the handler's result, error is absent, and `logs` carries the handler's console entries in the Cause 6 format.
Emulator: outErrors is absent, evaluationResult is absent, logs is [], and error = {message: "cannot read property '<name>' of undefined"}.
Why: util.appendError is how a resolver reports non-fatal field errors alongside a result; a client validating that behavior sees no appended errors at all.
Fix: as Cause 6, plus wiring util.appendError into EvaluateCodeResponse$outErrors.
Traces: appsync-eval-js-transformations#task#2.

=== Cause 8 of 10: a response-side handler reading its context fails the same way (fixture appsync-eval-js-transformations, step task#3) ===
AWS: recorded behavior only — HTTP 200 with evaluationResult set to the JSON the handler returns, error absent, `logs` omitted (nothing logged), stash "{}" and outErrors "[]".
Emulator: error = {message: "cannot read property '<name>' of undefined"}, evaluationResult absent, logs [], stash and outErrors absent.
Why: the unbound context is not confined to one handler kind — every evaluation in the corpus that reads a context member fails, so the operation cannot evaluate real resolver code at all.
Fix: as Cause 1.
Traces: appsync-eval-js-transformations#task#3.

=== Cause 9 of 10: a parse error is turned into an HTTP 400 refusal instead of a 200 carrying PARSE_ERROR codeErrors (fixture appsync-unit-resolver-code-evaluation, step task#4) ===
AWS: Smithy: EvaluateCode's http code is 200 and EvaluateCodeResponse$error carries codeErrors; BadRequestException(400) is modeled for malformed REQUESTS, not for code that does not compile. Recorded behavior: code that fails to parse is answered with HTTP 200, evaluationResult absent, and error = {message: "Error: code.js(<line>,<column>): error TS<code>: <compiler text>\n [code: PARSE_ERROR] [start: line: <line>, col: <column>] [end: line: <line>, col: <column>]", codeErrors: [{errorType: "PARSE_ERROR", value: "code.js(<line>,<column>): error TS<code>: <compiler text>\n", location: {line, column, span}}]} — the "error TS<code>:" shape and the [code: ...] / [start: ...] / [end: ...] suffixes are fixed; the file name, line, column, span and compiler text come from the submitted `code`.
Emulator: HTTP 400 — the response has no `error` member and the 200-with-diagnostics contract never happens.
Why: a client linting resolver code catches an exception instead of receiving the line/column diagnostics it asked for, and cannot distinguish "your code has a syntax error" from "the service rejected my request".
Fix: the EvaluateCode handler's compile/error path — compile failures must serialize into EvaluateCodeResponse$error with a 200 status instead of raising BadRequestException.
Traces: appsync-unit-resolver-code-evaluation#task#4.

=== Cause 10 of 10: even a fully correct evaluation omits stash and outErrors (fixture appsync-unit-resolver-code-evaluation, step task#5) ===
AWS: Smithy: EvaluateCodeResponse$stash and $outErrors. Recorded behavior: a successful evaluation returns stash "{}" and outErrors "[]" when the handler set no stash keys and appended no errors — the members are always present, never dropped.
Emulator: both members are absent, although evaluationResult matches.
Why: this isolates the envelope defect from the context defect — code that the emulator evaluates correctly still produces a response shape no AWS SDK model of EvaluateCode ever returns, so a client that reads stash to chain pipeline stages sees nothing.
Fix: the EvaluateCode handler's response projection/serializer (always emit stash and outErrors).
Traces: appsync-unit-resolver-code-evaluation#task#5.

## Evidence
All ten are mismatch clusters from trace set simparity-wire-aws-v1 (service appsync, operation EvaluateCode), none carrying stub_path or owned_by.

Cause 1 — appsync-eval-js-error-surface#task#1 (+task#2), expected 200 / got 200: $.error.message value_mismatch (expected 'id is required', got "cannot read property 'id' of undefined"); $.logs type_mismatch (expected absent, got []); $.stash missing (expected '{}', got None); $.outErrors missing (expected '[]', got None).
Cause 2 — appsync-eval-js-error-surface#task#3, expected 200 / got 200: $.evaluationResult type_mismatch (expected absent, got '{}'); $.error missing — expected {'message': 'Error: code.js(2,3): error @aws-appsync/no-for: For statements are not supported. [code: LINT_ERROR] [start: line: 2, col: 3] [end: line: 2, col: 33] Error: code.js(2,26): error @aws-appsync/no-disallowed-unary-operators: The operator ++ is not supported [code: LINT_ERROR] [start: line: 2, col: 26] [end: line: 2, col: 29]', 'codeErrors': [{'errorType': 'LINT_ERROR', 'value': 'code.js(2,3): error @aws-appsync/no-for: For statements are not supported.', 'location': {'line': 2, 'column': 3, 'span': 30}}, {'errorType': 'LINT_ERROR', 'value': 'code.js(2,26): error @aws-appsync/no-disallowed-unary-operators: The operator ++ is not supported', 'location': {'line': 2, 'column': 26, 'span': 3}}]}, got None; $.logs type_mismatch (expected absent, got []).
Cause 3 — appsync-eval-js-error-surface#task#4: status expected 400, got 200; $.message missing (expected 'Function must be provided', got None); $headers.x-amzn-errortype missing.
Cause 4 — appsync-eval-js-error-surface#task#5, 400/400: $.message value_mismatch (expected 'Runtime version invalid.', got 'Invalid runtimeVersion given.').
Cause 5 — appsync-eval-js-error-surface#task#6, 200/200: $.evaluationResult missing (expected '{"echo":"ok","n":1}', got None); $.error type_mismatch (expected absent, got {'message': "cannot read property 'id' of undefined"}); $.stash missing (expected '{}'); $.outErrors missing (expected '[]').
Cause 6 — appsync-eval-js-transformations#task#1, 200/200: $.evaluationResult missing (expected '{"operation":"PutItem","key":{"id":{"S":"order-1"}},"attributeValues":{"total":{"N":26},"skus":{"L":[{"S":"A1x2"},{"S":"C3x5"}]},"active":{"BOOL":true}}}', got None); $.error type_mismatch (got {'message': "cannot read property 'items' of undefined"}); $.logs[0] missing (expected 'INFO - code.js:5:3: "kept" 2 "of" 3'); $.logs[1] missing (expected 'INFO - code.js:6:3: {"total":26}'); $.logs[2] missing (expected 'ERROR - code.js:7:3: "discount" 10'); $.stash missing (expected '{"skus":["A1x2","C3x5"],"total":26}'); $.outErrors missing (expected '[]').
Cause 7 — appsync-eval-js-transformations#task#2, 200/200: $.evaluationResult missing (expected '{"count":0}'); $.error type_mismatch (got {'message': "cannot read property 'q' of undefined"}); $.logs[0] missing (expected 'INFO - code.js:10:3: "response for" "zzz"'); $.stash missing (expected '{}'); $.outErrors missing (expected '[{"message":"no rows matched","type":"EmptyResult","data":{"query":"zzz"},"info":{"hint":"widen"}},{"message":"second","type":"Other","data":null,"info":null}]').
Cause 8 — appsync-eval-js-transformations#task#3, 200/200: $.evaluationResult missing (expected '{"hit":true,"id":"k9"}'); $.error type_mismatch (got {'message': "cannot read property 'cached' of undefined"}); $.logs type_mismatch (expected absent, got []); $.stash missing (expected '{}'); $.outErrors missing (expected '[]').
Cause 9 — appsync-unit-resolver-code-evaluation#task#4: status expected 200, got 400; $.error missing — expected {'message': "Error: code.js(3,1): error TS1005: '}' expected.\n [code: PARSE_ERROR] [start: line: 3, col: 1] [end: line: 3, col: 1]", 'codeErrors': [{'errorType': 'PARSE_ERROR', 'value': "code.js(3,1): error TS1005: '}' expected.\n", 'location': {'line': 3, 'column': 1, 'span': 0}}]}, got None.
Cause 10 — appsync-unit-resolver-code-evaluation#task#5, 200/200: $.stash missing (expected '{}', got None); $.outErrors missing (expected '[]', got None).

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1); each cause below is one recorded step.

Cause 1: replay appsync-eval-js-error-surface step task#1 — appsync.evaluate_code(runtime={'name':'APPSYNC_JS','runtimeVersion':'1.0.0'}, code=<handler that reads a context member and calls util.error('id is required') when it is absent>, context=<the fixture's context JSON>, function='request'); compare error.message, and check that stash/outErrors are present and that logs is absent.
Cause 2: replay appsync-eval-js-error-surface task#3 — submit code containing a `for` statement and a `++` operator; expect 200 with error.codeErrors[*].errorType == 'LINT_ERROR' and location.line/column, observe 200 with evaluationResult '{}' and no error.
Cause 3: replay appsync-eval-js-error-surface task#4 — same call with the `function` member omitted; expect 400 'Function must be provided', observe 200.
Cause 4: replay appsync-eval-js-error-surface task#5 — runtime={'name':'APPSYNC_JS','runtimeVersion':<unsupported>}; expect 400 'Runtime version invalid.', observe 400 'Invalid runtimeVersion given.'.
Causes 5-8: replay appsync-eval-js-error-surface task#6 and appsync-eval-js-transformations task#1, task#2, task#3 and compare evaluationResult / error / logs / stash / outErrors against the recorded response.
Cause 9: replay appsync-unit-resolver-code-evaluation task#4 — code with an unbalanced brace; expect 200 with error.codeErrors[0].errorType 'PARSE_ERROR', observe HTTP 400.
Cause 10: replay appsync-unit-resolver-code-evaluation task#5 — a handler the emulator evaluates correctly; the response still omits stash and outErrors.
