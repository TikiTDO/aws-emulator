Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body|_await#8|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage|run#10|AssertionError|assert headers.get("x-amzn-errortype") == "InternalServerErrorException", (
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|run#11|AssertionError|assert (got["status"], _json(got["body"])) == (500, {"message": "Internal server error"}), (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# apigateway__dynamodb__iam__lambda: 3 net-new causes — DynamoDB never refuses the function role's GetItem, the stage's 500 carries no x-amzn-ErrorType, and TestInvokeMethod ignores the backend's resource policy

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Three independent root causes in the REST-API / IAM / DynamoDB path. All three checks are listed under `### apigateway__dynamodb__iam__lambda` in the behavioral suite and none is marked ALREADY FILED or OWNED. The open issue 20261105T223150Z-apigateway-dynamodb-iam-lambda-an-execute-api-iam.md covers a sibling check (the execute-api IAM lifecycle) and does NOT cover any of the three causes below.

Cause 1 of 3: a function's assumed-role session is not authorized against DynamoDB, so withdrawing the role's read policy changes nothing.
AWS: DynamoDB evaluates the caller's IAM identity on every data-plane call. Once `iam:DeleteRolePolicy` removes the function role's `dynamodb:GetItem` grant, `GetItem` is refused `400 AccessDeniedException` with the implicit-deny sentence `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: dynamodb:GetItem on resource: <table-arn> because no identity-based policy allows the dynamodb:GetItem action` — fixed text with the principal ARN, the action and the table ARN taken from the request and from IAM state. The proxy integration surfaces the handler's unhandled exception as a 500 whose body names `dynamodb:GetItem` and that principal (recorded behavior for the handler's own `except` path). Smithy: `GetItem` errors are `ResourceNotFoundException`, `ProvisionedThroughputExceededException`, `RequestLimitExceeded`, `InternalServerError`; `AccessDeniedException` is the service-common IAM refusal.
Emulator: after the role policy is deleted the same signed GET keeps returning `200` with the seeded item, so the expected 500 never arrives and the check exhausts its 180 s budget (`Error: Check timed out after 180.0s`). DynamoDB answers the function's session as if it were still granted.
Why: a client cannot test or rely on least-privilege scoping of a Lambda's DynamoDB access — a revoked grant silently keeps working, so an authorization regression is invisible and a negative test that must fail passes.
Fix: the DynamoDB data-plane handlers' authorization step (identity-policy evaluation for the caller's assumed-role session) must be consulted before the item read.
Traces: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body.

Cause 2 of 3: the stage's 500 for an un-invokable backend omits the `x-amzn-ErrorType` response header.
AWS: when API Gateway may no longer invoke the integration target (the function's `apigateway.amazonaws.com` grant for that stage was dropped with `lambda:RemovePermission`), the stage answers `500` with body `{"message": "Internal server error"}` AND the response header `x-amzn-ErrorType: InternalServerErrorException`. The header name and its value are fixed text; nothing in it varies per request.
Emulator: the 500 and the body are produced, but `x-amzn-ErrorType` is absent — `AssertionError: x-amzn-ErrorType was None`.
Why: SDKs and clients classify an execute-api failure from `x-amzn-ErrorType`; with the header missing the failure is an untyped 500 and retry/alarm logic keyed on `InternalServerErrorException` never fires.
Fix: the execute-api data-plane error serializer must emit `x-amzn-ErrorType` alongside the 500 body for integration-failure responses.
Traces: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage.

Cause 3 of 3: TestInvokeMethod does not consult the backend function's resource policy.
AWS: `TestInvokeMethod` bypasses the method's AWS_IAM authorizer but NOT the integration target's resource policy. With the function's only `lambda:InvokeFunction` statement conditioned on `ArnLike AWS:SourceArn = <api-arn>/<stage>/GET/<resource>/*`, the test invoke (whose source ARN is the test-invoke-stage ARN) is not permitted, so the test invoke reports `status` 500 with body `{"message": "Internal server error"}`; adding the test-invoke-stage grant makes the same call return the item with `status` 200. Smithy: `TestInvokeMethod` (GET /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}) returns `status`, `body`, `headers`, `log`, `latency`; errors `BadRequestException`, `NotFoundException`, `UnauthorizedException`, `ConflictException`, `LimitExceededException`.
Emulator: the test invoke without a test-invoke-stage grant returns `200` with the DynamoDB item, i.e. the function's resource policy is never evaluated for the test-invoke source ARN.
Why: the documented difference between the stage data plane and TestInvokeMethod disappears — a client using TestInvokeMethod to validate a stage-scoped permission gets a false pass, and a permission bug reaches production.
Fix: the `TestInvokeMethod` handler's integration-invocation step must evaluate the target function's resource policy against the test-invoke source ARN, exactly as the stage data plane does.
Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Cause 1 — expected: the GET becomes 500 whose body names dynamodb:GetItem and the function's assumed-role principal within the deadline. Actual: `Error: Check timed out after 180.0s`; the signed GET keeps answering 200 with the seeded agreement item after `iam:DeleteRolePolicy` removed the TableRead policy.

Cause 2 — expected: `x-amzn-ErrorType: InternalServerErrorException` on the 500. Actual: `AssertionError: x-amzn-ErrorType was None` (status 500 and body `{"message": "Internal server error"}` were correct).

Cause 3 — expected: `(status, body) == (500, {"message": "Internal server error"})` from TestInvokeMethod while the function grants only `<api-arn>/beta/GET/data-agreements/*`. Actual: `AssertionError: the test invoke without a test-invoke-stage grant returned 200 '{"agreementId": "dua-benchmark-001", ...}'`.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST DynamoDB table, a python3.12 Lambda that GetItems from it through a role carrying the CDK grantReadData actions, and a REGIONAL REST API with one AWS_IAM-authorized, Lambda-proxied `GET /data-agreements/{agreementId}` deployed to stage `beta`.
2. Cause 1: confirm a SigV4-signed GET returns 200, then `iam:DeleteRolePolicy` the function role's read policy and re-issue the signed GET — it must become 500 naming dynamodb:GetItem; the emulator keeps answering 200.
3. Cause 2: `lambda:RemovePermission` the `apigateway.amazonaws.com` statement for stage `beta`, re-issue the signed GET and read the response headers — `x-amzn-ErrorType` must be `InternalServerErrorException`.
4. Cause 3: provision the same estate with the function's invoke permission conditioned only on `<api-arn>/beta/GET/data-agreements/*`, then call `apigateway:TestInvokeMethod` on that method — it must report status 500, not 200.
