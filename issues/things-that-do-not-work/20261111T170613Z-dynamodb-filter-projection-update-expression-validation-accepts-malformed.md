Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.query-filter-expression-between-reversed-bounds-rejected dynamodb.scan-filter-between-binary-attribute-reversed-bounds-rejected dynamodb.query-filter-expression-redundant-parentheses-rejected dynamodb.scan-filter-expression-redundant-nested-parens-rejected dynamodb.scan-filter-expression-ordering-comparison-non-orderable-operand-rejected dynamodb.scan-and-query-filter-ordering-comparison-on-a-set-attribute-rejected dynamodb.scan-filter-size-covers-s-b-ss-l-m-not-n dynamodb.scan-filter-legacy-in-with-a-large-operand-list dynamodb.update-remove-add-delete-through-invalid-path-rejected dynamodb.update-set-arithmetic-and-list-append-type-mismatch-rejected dynamodb.update-item-chained-arithmetic-operators-on-list-index-rejected dynamodb.projection-negative-list-index-rejected-across-read-operations
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.query-filter-expression-between-reversed-bounds-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.scan-filter-between-binary-attribute-reversed-bounds-rejected|_error_code_and_message#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.query-filter-expression-redundant-parentheses-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.scan-filter-expression-redundant-nested-parens-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.scan-filter-expression-ordering-comparison-non-orderable-operand-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.scan-and-query-filter-ordering-comparison-on-a-set-attribute-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb|Scan|ValidationException
Issue-Key: dynamodb.update-remove-add-delete-through-invalid-path-rejected|_error#6|AssertionError|raise AssertionError(f"call unexpectedly SUCCEEDED where an error was required: {resp!r}")
Issue-Key: dynamodb.update-set-arithmetic-and-list-append-type-mismatch-rejected|run#8|AssertionError|assert message == _TYPE_MISMATCH, (
Issue-Key: dynamodb.update-item-chained-arithmetic-operators-on-list-index-rejected|run#11|AssertionError|assert message == _MSG_CHAINED_OP, f"the refusal's message is {message!r}, expected {_MSG_CHAINED_OP!r}"
Issue-Key: dynamodb.projection-negative-list-index-rejected-across-read-operations|expect_error#6|AssertionError|assert got == (code, message), f"got {got!r}, expected {(code, message)!r}"
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: Filter/Projection/Update expression validation accepts malformed expressions and mis-reports operand and path defects (10 causes)

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Ten net-new root causes in the expression engine's validation of FilterExpression, ProjectionExpression and UpdateExpression. Smithy models none of this text: every one of these is a 400 ValidationException whose message is the contract, and in six of the ten the emulator serves no refusal at all and answers 200. In each quoted message the attribute names, operator and operand type come from the request; the surrounding sentence is AWS's fixed text.

Cause 1 of 10 — BETWEEN with reversed bounds in a FilterExpression is accepted. AWS refuses it with ValidationException; the emulator runs the scan/query and returns a (usually empty) result set. Seen for N bounds on Query and for B bounds on Scan. Why: a client with a transposed range silently gets 'no matches' instead of the error that would have revealed the bug. Fix: BETWEEN operand-order validation in the expression engine. Traces: query-filter-expression-between-reversed-bounds-rejected, scan-filter-between-binary-attribute-reversed-bounds-rejected.

Cause 2 of 10 — redundant parentheses in a FilterExpression are accepted. AWS refuses both the single-redundant and the nested-redundant form; the emulator evaluates the filter and returns items. Fix: the expression parser's redundant-grouping rule. Traces: query-filter-expression-redundant-parentheses-rejected, scan-filter-expression-redundant-nested-parens-rejected.

Cause 3 of 10 — an ordering comparison (<, <=, >, >=) against a non-orderable operand is accepted. AWS refuses it; the emulator scans and returns Count 0. Fix: operand-type checking for ordering comparisons. Traces: scan-filter-expression-ordering-comparison-non-orderable-operand-rejected.

Cause 4 of 10 — an ordering comparison against a SET attribute is accepted on both Scan and Query. AWS refuses it. Fix: the same operand-type check, for set-typed attributes. Traces: scan-and-query-filter-ordering-comparison-on-a-set-attribute-rejected.

Cause 5 of 10 — size() over S, B, SS, L and M in a FilterExpression is refused as a syntax error. AWS accepts size() over all of those (and refuses it only over N). Emulator: ValidationException 'Invalid FilterExpression: Syntax error; token: "=", near: ") = :n"'. Fix: the size() function's grammar/type table. Traces: scan-filter-size-covers-s-b-ss-l-m-not-n.

Cause 6 of 10 — the legacy ScanFilter IN comparator with a large operand list is refused. AWS accepts it; the emulator answers ValidationException 'One or more parameter values were invalid: Invalid number of argument(s) for the IN ComparisonOperator'. Fix: the legacy ScanFilter IN arity check. Traces: scan-filter-legacy-in-with-a-large-operand-list.

Cause 7 of 10 — an invalid document path in an UpdateExpression REMOVE/ADD/DELETE clause is accepted and the call answers 200. AWS refuses the update. Fix: document-path validation in the UpdateItem path resolver. Traces: update-remove-add-delete-through-invalid-path-rejected.

Cause 8 of 10 — the UpdateExpression arithmetic / list_append type-mismatch refusal uses the wrong message. AWS serves the bare 'An operand in the update expression has an incorrect data type'; the emulator serves the constraint-style '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator or function: +, operand type: S'. Fix: the update-operand type-mismatch diagnostic. Traces: update-set-arithmetic-and-list-append-type-mismatch-rejected.

Cause 9 of 10 — the UpdateExpression syntax-error diagnostic points at the wrong token. AWS names the token where the chain actually breaks ('Invalid UpdateExpression: Syntax error; token: "-", near: ") - :y"'); the emulator names the opening paren of the clause ('... token: "(", near: "= (a"'). Fix: the UpdateExpression parser's error-position reporting. Traces: update-item-chained-arithmetic-operators-on-list-index-rejected.

Cause 10 of 10 — the ProjectionExpression negative-list-index refusal points at the wrong token. AWS: 'Invalid ProjectionExpression: Syntax error; token: "-", near: "[-1"'; the emulator: '... token: "[", near: "#l[-1]"'. Why (causes 8-10): the token and the 'near:' window are how a developer locates the defect in the expression they sent; naming a different token points at valid text. Fix: the ProjectionExpression parser's error-position reporting. Traces: projection-negative-list-index-rejected-across-read-operations.

## Evidence
Expected vs actual, verbatim from the replay:
- query-filter-expression-between-reversed-bounds-rejected / scan-filter-between-binary-attribute-reversed-bounds-rejected / scan-filter-expression-ordering-comparison-non-orderable-operand-rejected / scan-and-query-filter-ordering-comparison-on-a-set-attribute-rejected: 'expected the call to be refused, but it succeeded' with HTTP 200 and an Items/Count body.
- query-filter-expression-redundant-parentheses-rejected / scan-filter-expression-redundant-nested-parens-rejected: 'expected the call to be rejected, but it succeeded' with HTTP 200 and matching items returned.
- scan-filter-size-covers-s-b-ss-l-m-not-n: ClientError (ValidationException) 'Invalid FilterExpression: Syntax error; token: "=", near: ") = :n"' for a size() form AWS accepts.
- scan-filter-legacy-in-with-a-large-operand-list: ClientError (ValidationException) 'One or more parameter values were invalid: Invalid number of argument(s) for the IN ComparisonOperator' where AWS runs the scan.
- update-remove-add-delete-through-invalid-path-rejected: 'call unexpectedly SUCCEEDED where an error was required' (HTTP 200, empty body).
- update-set-arithmetic-and-list-append-type-mismatch-rejected: refusal message is '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator or function: +, operand type: S', expected 'An operand in the update expression has an incorrect data type'.
- update-item-chained-arithmetic-operators-on-list-index-rejected: got '1 validation error detected: Invalid UpdateExpression: Syntax error; token: "(", near: "= (a"', expected '1 validation error detected: Invalid UpdateExpression: Syntax error; token: "-", near: ") - :y"'.
- projection-negative-list-index-rejected-across-read-operations: got ('ValidationException', 'Invalid ProjectionExpression: Syntax error; token: "[", near: "#l[-1]"'), expected ('ValidationException', 'Invalid ProjectionExpression: Syntax error; token: "-", near: "[-1"').

## Steps to Reproduce
1. Create a table and seed items carrying S, B, N, SS, L and M attributes.
2. Query/Scan with FilterExpression 'a BETWEEN :hi AND :lo' (bounds transposed, once with N and once with B) — expect ValidationException.
3. Query/Scan with a FilterExpression carrying redundant and nested-redundant parentheses — expect ValidationException.
4. Scan with FilterExpression 'a < :v' where :v is non-orderable, and with an ordering comparison against an SS attribute — expect ValidationException.
5. Scan with FilterExpression 'size(a) = :n' for a in {S, B, SS, L, M} — expect success; for an N attribute — expect refusal.
6. Scan with a legacy ScanFilter IN comparator holding a large AttributeValueList — expect success.
7. UpdateItem with REMOVE/ADD/DELETE through an invalid document path — expect ValidationException.
8. UpdateItem with 'SET word = word + :one' (S attribute) and with chained arithmetic on a list index; read the message and the reported token.
9. GetItem/Query/Scan with ProjectionExpression '#l[-1]'; read the reported token and 'near:' window.
