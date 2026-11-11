Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-partiql-is-type-name-filters-bool-and-string-attributes dynamodb.ddb-partiql-is-type-name-rejects-update-leaves-item-unchanged dynamodb.execute-statement-number-literal-exponent-and-leading-dot-forms dynamodb.execute-statement-literal-leading-dot-with-no-mantissa-digit-rejected dynamodb.partiql-line-and-block-comments-ignored-order-by-asc dynamodb.partiql-hash-and-unterminated-block-comment-rejected dynamodb.partiql-single-quote-literal-escaping dynamodb.partiql-select-condition-function-forms dynamodb.partiql-update-from-first-clause-order-runs-as-write dynamodb.execute-statement-limit-clause-rejected dynamodb.execute-statement-missing-literal-comparison-operators-rejected dynamodb.partiql-unquoted-non-literal-token-as-value-rejected dynamodb.execute-statement-quoting-rules
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExecuteStatement|ValidationException
Issue-Key: dynamodb.partiql-hash-and-unterminated-block-comment-rejected|expect_error#10|AssertionError|assert got_message == message, f"expected message {message!r}, got {got_message!r}"
Issue-Key: dynamodb.execute-statement-limit-clause-rejected|run#14|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.execute-statement-missing-literal-comparison-operators-rejected|run#5|AssertionError|assert got == _REFUSAL, (
Issue-Related: 20261105T114920Z-dynamodb-6-net-new-partiql-and-updateexpression-causes.md
Issue-Commit: cdd06381ed07ce191d4321618e22d6aefb363f32

# dynamodb: PartiQL statement parser rejects AWS's grammar and collapses 10 distinct refusals into generic parse text

> _Note: this issue was identified against the code base at commit `cdd06381ed07ce191d4321618e22d6aefb363f32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Ten net-new root causes in the PartiQL statement parser used by ExecuteStatement / BatchExecuteStatement / ExecuteTransaction. In every case the emulator either refuses a statement AWS accepts, or refuses with the parser's generic fallback text instead of the specific ValidationException message AWS words for that defect. Smithy: ExecuteStatement/BatchExecuteStatement/ExecuteTransaction model no statement-syntax error shape, so every one of these is a 400 ValidationException whose message is the whole contract; the model states only Statements @length 1..25 (BatchExecuteStatement) and TransactStatements @length 1..100 (ExecuteTransaction). Message rule for all causes below: the fixed text is AWS's; only the attribute/type/index names and the line:col:offset come from the request.

Cause 1 of 10 — `IS <type-name>` predicate is not in the grammar. AWS: `WHERE attr IS BOOL` / `IS NOT MISSING` are legal PartiQL predicates and run as a filter (SELECT) or as an update condition. Emulator: ValidationException "Statement wasn't well formed, can't be processed: Expected keyword" on the first `IS` token. Why: a client cannot type-filter at all, and an UPDATE guarded by `IS` fails instead of leaving the item unchanged. Fix: the PartiQL grammar/predicate parser. Traces: ddb-partiql-is-type-name-filters-bool-and-string-attributes, ddb-partiql-is-type-name-rejects-update-leaves-item-unchanged.

Cause 2 of 10 — number literal forms. AWS: exponent (`1e5`, `1E-3`) and leading-dot (`.5`) number literals are accepted; a leading dot with no mantissa digit at all is refused with the parser's own specific message. Emulator: every one of these forms answers ValidationException "Statement wasn't well formed, can't be processed: Unexpected term". Why: legal statements are unusable, and the one genuinely bad literal is indistinguishable from them. Fix: the PartiQL lexer's numeric-literal rule. Traces: execute-statement-number-literal-exponent-and-leading-dot-forms, execute-statement-literal-leading-dot-with-no-mantissa-digit-rejected.

Cause 3 of 10 — comments. AWS: `--` line comments and `/* */` block comments are stripped before parsing, so a commented statement runs; an unterminated block comment is refused with AWS's specific text (the recording shows a message ending at "can't be processed: " with no trailing diagnostic). Emulator: a commented statement answers ValidationException "Statement wasn't well formed, can't be processed: Unexpected token after expression", and the unterminated-comment refusal appends "Unexpected term". Fix: the PartiQL lexer's comment handling. Traces: partiql-line-and-block-comments-ignored-order-by-asc, partiql-hash-and-unterminated-block-comment-rejected.

Cause 4 of 10 — single-quote escaping inside string literals. AWS: `''` inside a single-quoted literal is one quote character; the malformed forms carry AWS's bare "Statement wasn't well formed, can't be processed: " text. Emulator: appends "Unexpected term". Fix: the PartiQL lexer's string-literal rule. Traces: partiql-single-quote-literal-escaping.

Cause 5 of 10 — condition function forms in a SELECT WHERE clause (begins_with/contains/attribute_type style forms AWS supports). Emulator: ValidationException "Statement wasn't well formed, can't be processed: Unexpected token after expression". Fix: the PartiQL WHERE-clause function grammar. Traces: partiql-select-condition-function-forms.

Cause 6 of 10 — UPDATE clause ordering. AWS: an UPDATE whose clauses appear in the other legal order still runs as a write. Emulator: ValidationException "Statement wasn't well formed, can't be processed: Unexpected token after expression". Fix: the PartiQL UPDATE grammar. Traces: partiql-update-from-first-clause-order-runs-as-write.

Cause 7 of 10 — LIMIT clause refusal text. AWS: refuses an unsupported LIMIT with "Unsupported clause: LIMIT at <line>:<col>:<len>" — the clause keyword and position come from the statement, the rest is fixed. Emulator: "Statement wasn't well formed, can't be processed: Unexpected token after expression". Fix: the PartiQL parser's unsupported-clause diagnostic. Traces: execute-statement-limit-clause-rejected.

Cause 8 of 10 — MISSING comparison. AWS: `"status" = MISSING` is refused with the fixed text "MISSING must be compared using IS operator". Emulator: "Statement wasn't well formed, can't be processed: Unexpected keyword". Fix: the PartiQL comparison-operand validation. Traces: execute-statement-missing-literal-comparison-operators-rejected.

Cause 9 of 10 — unquoted non-literal token used as a value. AWS: "Unsupported data type: <token> under key <key> at <line>:<col>:<len>" (token/key/position from the request; the rest fixed). Emulator: "Statement wasn't well formed, can't be processed: Unexpected term". Fix: the PartiQL value parser. Traces: partiql-unquoted-non-literal-token-as-value-rejected.

Cause 10 of 10 — keyword vs term classification for an unquoted reserved attribute name. AWS: "Statement wasn't well formed, can't be processed: Unexpected keyword". Emulator: "... Unexpected term". Why (all message causes): a client that branches on the refusal text, or surfaces it to a user, cannot tell one statement defect from another. Fix: the PartiQL lexer's reserved-word table. Traces: execute-statement-quoting-rules.

## Evidence
Expected vs actual, verbatim from the replay:
- ddb-partiql-is-type-name-*: ClientError (ValidationException) "Statement wasn't well formed, can't be processed: Expected keyword" where AWS runs the statement.
- execute-statement-number-literal-exponent-and-leading-dot-forms / ...-literal-leading-dot-with-no-mantissa-digit-rejected: ValidationException "Statement wasn't well formed, can't be processed: Unexpected term".
- partiql-line-and-block-comments-ignored-order-by-asc / partiql-select-condition-function-forms / partiql-update-from-first-clause-order-runs-as-write: ValidationException "Statement wasn't well formed, can't be processed: Unexpected token after expression".
- partiql-hash-and-unterminated-block-comment-rejected: expected message "Statement wasn't well formed, can't be processed: ", got "Statement wasn't well formed, can't be processed: Unexpected term".
- partiql-single-quote-literal-escaping: expected ('ValidationException', "Statement wasn't well formed, can't be processed: "), got ('ValidationException', "Statement wasn't well formed, can't be processed: Unexpected term").
- execute-statement-limit-clause-rejected: expected 'Unsupported clause: LIMIT at 1:51:1', got "Statement wasn't well formed, can't be processed: Unexpected token after expression".
- execute-statement-missing-literal-comparison-operators-rejected: expected ('ValidationException', 'MISSING must be compared using IS operator'), got ('ValidationException', "Statement wasn't well formed, can't be processed: Unexpected keyword").
- partiql-unquoted-non-literal-token-as-value-rejected: expected 'Unsupported data type: yes under key root at 1:82:3', got "Statement wasn't well formed, can't be processed: Unexpected term".
- execute-statement-quoting-rules: unquoted reserved attribute: expected "... Unexpected keyword", got "... Unexpected term".
(The line:col:offset and attribute names in the quoted expectations are values from those particular statements, not part of the fixed text.)

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table with a simple HASH key and seed a couple of items.
2. ExecuteStatement each of: a SELECT with `WHERE attr IS BOOL`; a statement carrying `1e5` and `.5` literals; a statement preceded by a `--` line comment and containing a `/* */` block comment; a SELECT whose WHERE clause uses a condition function; an UPDATE with the alternate clause order.
3. ExecuteStatement each refusal probe: a SELECT with `LIMIT 1`; `WHERE "status" = MISSING`; a value position holding an unquoted bare token; an unquoted reserved attribute name; an unterminated `/*` comment; a literal containing `''`.
4. Compare each response's error code and message against the fixed texts above.
