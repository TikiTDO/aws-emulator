Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.execute-statement-missing-clause-rejected dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.execute-statement-missing-clause-rejected|_assert_rejected#16|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Key: dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected|run#11|AssertionError|assert (code, msg) == ("ValidationException", want), f"{label} gave {code!r}: {msg!r}"
Issue-Commit: c607a2586d7f91da73a3e70d6fec1fead9553942

# dynamodb: 2 net-new ValidationException message causes — a non-DML PartiQL statement is reported as a parse error instead of "Unsupported operation: <Op>", and Query's legacy/expression mixing message lists the non-expression parameters in the wrong order

> _Note: this issue was identified against the code base at commit `c607a2586d7f91da73a3e70d6fec1fead9553942`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement, Query

## Description
Two independent message-fidelity root causes, each from one failing check. Neither is covered by the open DynamoDB PartiQL/expression issues the report names for other checks (20261029T180026Z-dynamodb-partiql-resource-policy-arn-addressing-crc32-header.md, 20261103T065250Z-dynamodb-expression-engine-10-root-causes-in-expression.md): those own different checks and different statements.

Cause 1 of 2 — ExecuteStatement with a non-DML leading keyword falls into the generic parse-error path.
AWS: DynamoDB's PartiQL surface supports only the DML statements SELECT / INSERT / UPDATE / DELETE. A statement whose leading keyword names a DDL/other operation (e.g. `DROP TABLE "<table>"`) is refused 400 ValidationException whose message is the fixed text "Unsupported operation: " followed by the name of the operation that leading keyword maps to — "DropTable" for DROP TABLE; the operation name is derived from the request, the prefix is fixed. Smithy: ExecuteStatement.Statement is PartiQLStatement, @length min 1 max 8192; ValidationException is not in ExecuteStatement's modeled error list (it is one of DynamoDB's common client errors), so the message text is recorded behavior only. The table must be left untouched (DescribeTable still ACTIVE).
Emulator: 400 ValidationException with "Statement wasn't well formed, can't be processed: Unexpected token after expression" — the DDL statement is routed through the generic PartiQL syntax-error path rather than classified as an unsupported operation. The same check's other legs (SELECT without FROM, UPDATE without SET/REMOVE, INSERT without VALUE, DELETE without WHERE) matched AWS, so only the non-DML classification diverges.
Why: a client cannot distinguish "my statement is malformed" from "DynamoDB PartiQL does not support this operation"; retry/report logic and error-string matching on the documented "Unsupported operation: ..." wording break.
Fix: the ExecuteStatement handler's statement classification/validation stage — recognise a non-DML leading keyword before parsing continues and refuse with "Unsupported operation: <Op>".
Traces: dynamodb.execute-statement-missing-clause-rejected.

Cause 2 of 2 — the legacy/expression mixing message lists Query's non-expression parameters in the wrong order.
AWS: a Query or Scan carrying both legacy (non-expression) and expression parameters is refused 400 ValidationException with the fixed text "Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {<legacy list>} Expression parameters: {<expression list>}". The braced contents are the parameters actually present in that request, and AWS emits each side in its own fixed order per operation; for a Query carrying KeyConditions + QueryFilter + ConditionalOperator together with KeyConditionExpression + FilterExpression + ProjectionExpression the non-expression side is `{QueryFilter, ConditionalOperator, KeyConditions}` and the expression side is `{ProjectionExpression, FilterExpression, KeyConditionExpression}`. Smithy: QueryInput carries AttributesToGet / KeyConditions / QueryFilter / ConditionalOperator alongside ProjectionExpression / FilterExpression / KeyConditionExpression; ValidationException is not in Query's modeled errors, so the ordering is recorded behavior only.
Emulator: the code and the sentence match, and the expression side matches, but the non-expression side is emitted as `{KeyConditions, QueryFilter, ConditionalOperator}` — i.e. in input/model member order rather than AWS's order. The Scan equivalent (`{AttributesToGet, ScanFilter, ConditionalOperator}`) matched, so the divergence is Query's non-expression ordering specifically.
Why: callers and tooling that compare the message for equality, or read the braced list positionally, misreport which parameter caused the refusal.
Fix: the Query request-validation stage that builds the mixing ValidationException message — order the non-expression names as AWS does instead of following the request/model member order.
Traces: dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected.

## Evidence
Cause 1: ExecuteStatement(Statement='DROP TABLE "<table>"') on an ACTIVE table.
  Expected: 400 ValidationException, Message = 'Unsupported operation: DropTable'.
  Got: 400 ValidationException, Message = "Statement wasn't well formed, can't be processed: Unexpected token after expression".
  (Fixed text is 'Unsupported operation: '; 'DropTable' is the operation named by the request's leading keyword.)

Cause 2: Query(TableName=<t>, KeyConditions={pk: EQ}, QueryFilter={a: EQ}, ConditionalOperator='AND', KeyConditionExpression='pk = :p', FilterExpression='a = :v', ProjectionExpression='b', ExpressionAttributeValues={...}).
  Expected: ValidationException 'Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {QueryFilter, ConditionalOperator, KeyConditions} Expression parameters: {ProjectionExpression, FilterExpression, KeyConditionExpression}'.
  Got: ValidationException 'Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {KeyConditions, QueryFilter, ConditionalOperator} Expression parameters: {ProjectionExpression, FilterExpression, KeyConditionExpression}'.
  Fixed text = the sentence plus the two 'Non-expression parameters: {...} Expression parameters: {...}' labels; the variable parts are the member names present in the request, emitted in AWS's per-operation order. The Scan case with AttributesToGet + ScanFilter + ConditionalOperator + ProjectionExpression + FilterExpression passed, so the serializer is right for Scan and wrong for Query.

## Steps to Reproduce
Cause 1: CreateTable(KeySchema pk HASH, BillingMode=PAY_PER_REQUEST), wait ACTIVE, ExecuteStatement(Statement='INSERT INTO "<t>" VALUE {\'pk\': \'seed\', \'n\': 1}') to seed, then ExecuteStatement(Statement='DROP TABLE "<t>"') and read Error.Code / Error.Message; DescribeTable must still report ACTIVE.
Cause 2: CreateTable as above, PutItem one item, then call Query with KeyConditions, QueryFilter, ConditionalOperator, KeyConditionExpression, FilterExpression, ProjectionExpression and ExpressionAttributeValues all set, and compare Error.Message to AWS's string character for character.
