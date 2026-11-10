Issue-Source: trace-sweep-divergence
Issue-Trace: ddb-sfa-traj-function-operand-mistakes-then-corrected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddb-sfa-traj-function-operand-mistakes-then-corrected ddb-sfa-traj-select-enum-casing-then-count ddb-sfa-traj-wrong-table-name-then-corrected dynamodb-exclusive-start-key-empty-attributevalue-rejected dynamodb-frctraj-legacy-scan-filter-mixing-and-casing dynamodb-frctraj-number-operand-malformed dynamodb-frctraj-parallel-scan-segment-mistakes dynamodb-query-scan-exclusive-start-key-empty-value dynamodb-scan-segment-above-and-at-total-segments-maximum dynamodb-sfr-traj-parallel-scan-segment-mistakes dynamodb-ssbs-traj-cli-query-paths-in-projection dynamodb-ssbs-traj-projection-request-shape-mistakes dynamodb-trajwf-parallel-scan-segment-contract dynamodb-trajwf-scan-limit-equals-item-count-pages-once-more dynamodb-trajwf-untyped-expression-values-rejected dynamodb-empty-expression-string-rejected dynamodb-expression-attribute-value-empty-rejected dynamodb-pagination-contracts dynamodb-query-key-condition-duplicate-sort-key-clause-and-nested-path dynamodb-query-key-condition-operand-type-vs-schema-type dynamodb-sfr-traj-query-key-condition-mistakes ddbval-nesting-depth-over-32-levels-rejected dynamodb-malformed-n-in-key-top-level-not-cancellation dynamodb-refarch-rekreup-stale-row-read-addressing dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|Scan|4xx->4xx|$.message
Issue-Key: dynamodb|Query|4xx->4xx|$.message
Issue-Key: dynamodb|GetItem|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateItem|4xx->4xx|$.message
Issue-Related: 20261104T061030Z-dynamodb-serves-the-wrong-error-class-for-malformed.md
Issue-Commit: b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca

# dynamodb expression diagnostics: 4 net-new causes — operand-count, empty-expression, nesting-depth and same-operand refusals diverge

> _Note: this issue was identified against the code base at commit `b1b0bc7f12f2b32e092da76cddcf605dbbe6d9ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
This bundle carries 4 net-new causes. All four are in the DynamoDB expression subsystem: the emulator's expression validator reports a generic parse/syntax failure (or does not validate at all) where AWS reports a specific, named semantic diagnostic. Each cause must be closed individually.

Cause 1 of 4 — FilterExpression function arity is reported as a syntax error (Scan; fixture ddb-sfa-traj-function-operand-mistakes-then-corrected step task#4).
AWS: Smithy: Scan.FilterExpression targets ConditionExpression; the refusal class is ValidationException (__type 'com.amazon.coral.validate#ValidationException'), HTTP 400. Recorded response message: 'Invalid FilterExpression: Incorrect number of operands for operator or function; operator or function: attribute_exists, number of operands: 2'. Fixed text of that class: 'Invalid <ExpressionMember>: Incorrect number of operands for operator or function; operator or function: ', then ', number of operands: '. Variable parts, taken from the request: the member name being validated, the function/operator name as written in the expression, and the count of operands actually supplied.
Emulator: HTTP 400, but message 'Invalid FilterExpression: Syntax error; token: ",", near: "#a, #b"'.
Why: a caller that passed a grammatically valid expression with the wrong operand count is told its expression does not parse, so the SDK/CLI user (and any client matching on the arity diagnostic) looks for a quoting or grammar problem that does not exist, and the corrective edit AWS points at (drop the extra operand) is never indicated.
Fix: Scan/Query FilterExpression validation — the function-arity check must run after a successful parse and own the diagnostic, instead of the parser rejecting a well-formed argument list.

Cause 2 of 4 — an empty KeyConditionExpression is reported as an <EOF> syntax error (Query; fixture dynamodb-empty-expression-string-rejected step task#7).
AWS: Smithy: Query.KeyConditionExpression targets KeyExpression; refusal is ValidationException, HTTP 400. Recorded message: 'Invalid KeyConditionExpression: The expression can not be empty;' — the whole sentence (including the trailing semicolon) is fixed text for this class; the only variable part is the expression member name.
Emulator: HTTP 400 with 'Invalid KeyConditionExpression: Syntax error; token: "<EOF>", near: ""'.
Why: the empty-string case is the single most common client defect (an unset template variable). AWS names it explicitly; the emulator reports an internal parser token (<EOF>) that no AWS response ever contains, so error-message assertions and user-facing surfacing of the real cause both fail.
Fix: Query/Scan expression validation — reject empty/whitespace-only expression strings with the dedicated empty-expression diagnostic before handing the string to the parser.

Cause 3 of 4 — the ProjectionExpression nesting-depth refusal is emulator-authored prose (GetItem; fixture ddbval-nesting-depth-over-32-levels-rejected step task#8).
AWS: Smithy: GetItem.ProjectionExpression; refusal is ValidationException, HTTP 400. Recorded message: 'Invalid ProjectionExpression: The document path has too many nesting levels; nesting levels: 33'. Fixed text: 'Invalid <ExpressionMember>: The document path has too many nesting levels; nesting levels: '; the variable part is the depth of the offending document path as counted from the request.
Emulator: HTTP 400 with 'Invalid ProjectionExpression: a document path may name at most 32 levels' — prose the service never emits, stating the limit instead of the observed depth.
Why: the response does not tell the caller how deep its path actually was, and clients/tests that match AWS's sentence see an unrecognised message for a correctly detected condition.
Fix: the projection/document-path validator shared by GetItem/Query/Scan — emit AWS's sentence and report the counted nesting level of the rejected path.

Cause 4 of 4 — contains(<path>, <same path>) is evaluated instead of refused, so the wrong error class is returned (UpdateItem; fixture dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected step task#4).
AWS: Smithy: UpdateItem errors include ConditionalCheckFailedException and the service-common ValidationException. A ConditionExpression whose first operand is not distinct from the remaining operands is a request-shape defect, refused before evaluation: __type 'com.amazon.coral.validate#ValidationException', HTTP 400, message '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: contains, first operand: [tagset]'. Fixed text: '<N> validation error detected: Invalid <ExpressionMember>: The first operand must be distinct from the remaining operands for this operator or function; operator: ', then ', first operand: '. Variable parts from the request: the operator/function name and the first operand's document path, rendered in brackets.
Emulator: HTTP 400 but __type 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException' with message 'The conditional request failed' — it evaluates the condition and reports a conditional-check failure.
Why: this is a wrong error class, not just wrong text. ConditionalCheckFailedException is a retry-never/'someone else won' signal that SDK clients and application code handle as expected contention, so a malformed expression is silently absorbed as a lost race: the caller's write is skipped forever and the actual bug (a self-referential contains) is never reported. Code that branches on ValidationException to fail fast never runs.
Fix: ConditionExpression validation for UpdateItem/PutItem/DeleteItem (and the Transact* variants that share it) — the distinct-operand rule must be enforced in the validation pass that precedes condition evaluation, and raise ValidationException.

Related open issues (neither covers these clusters; the replay reports no owner for them): '20261109T172142Z-dynamodb-7-net-new-causes-validationexception-message-text.md' and '20261110T155738Z-dynamodb-5-net-new-causes-putitem-s-missing.md' cover other expression/validation message divergences.

## Evidence
Cause 1 (Scan, ddb-sfa-traj-function-operand-mistakes-then-corrected#task#4, 23 affected steps): expected 400 / $.message 'Invalid FilterExpression: Incorrect number of operands for operator or function; operator or function: attribute_exists, number of operands: 2'; actual 400 / 'Invalid FilterExpression: Syntax error; token: ",", near: "#a, #b"'.
Cause 2 (Query, dynamodb-empty-expression-string-rejected#task#7, 19 affected steps): expected 400 / 'Invalid KeyConditionExpression: The expression can not be empty;'; actual 400 / 'Invalid KeyConditionExpression: Syntax error; token: "<EOF>", near: ""'.
Cause 3 (GetItem, ddbval-nesting-depth-over-32-levels-rejected#task#8, 3 affected steps): expected 400 / 'Invalid ProjectionExpression: The document path has too many nesting levels; nesting levels: 33'; actual 400 / 'Invalid ProjectionExpression: a document path may name at most 32 levels'.
Cause 4 (UpdateItem, dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected#task#4, 2 affected steps): expected 400 / __type 'com.amazon.coral.validate#ValidationException' + message '1 validation error detected: Invalid ConditionExpression: The first operand must be distinct from the remaining operands for this operator or function; operator: contains, first operand: [tagset]'; actual 400 / __type 'com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException' + message 'The conditional request failed'.

## Steps to Reproduce
1. Replay fixture ddb-sfa-traj-function-operand-mistakes-then-corrected step task#4: Scan with FilterExpression 'attribute_exists(#a, #b)' (a function given two operands) — compare $.message.
2. Replay dynamodb-empty-expression-string-rejected step task#7: Query with KeyConditionExpression '' — compare $.message.
3. Replay ddbval-nesting-depth-over-32-levels-rejected step task#8: GetItem with a ProjectionExpression document path 33 levels deep — compare $.message.
4. Replay dynamodb-condition-expression-contains-same-path-and-between-reversed-rejected step task#4: UpdateItem with ConditionExpression 'contains(#tagset, #tagset)' — compare $.__type and $.message.
5. Re-replay the whole corpus: the four clusters above (and their 47 affected steps) must pass.
