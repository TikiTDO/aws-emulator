Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format apigateway__dynamodb__lambda__logs.apigw-execution-logs-delivered-to-cloudwatch apigateway__dynamodb__lambda__logs.apigw-http-api-access-logs-json-format apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|run#27|AssertionError|assert denied_result["status"] == 500, f"expected 500 without permission, got {denied_result}"
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage|_poll_log_events#16|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format|_poll_log_events_with_traffic#23|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-execution-logs-delivered-to-cloudwatch|_poll_log_events_with_traffic#14|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-access-logs-json-format|_poll_for_json_access_log#24|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative|_poll_log_events_with_traffic#22|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip|_wait_for_log_marker#30|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test|_wait_for_log_marker#30|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative|_wait_for_log_marker#31|Timeout|time.sleep(interval_s)
Issue-Commit: 48b98241475aee57ea7962587dcd25c49f07ef51

# apigateway__dynamodb__lambda__logs: 3 net-new causes — an AWS_PROXY integration invokes a Lambda with no resource-policy grant and answers 200, REST stage access/execution logs never reach their destination, and CLI create→deploy→invoke roundtrips never complete

> _Note: this issue was identified against the code base at commit `48b98241475aee57ea7962587dcd25c49f07ef51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Three root causes carried by the checks of this service that no open issue claims.

Cause 1 of 3: API Gateway does not enforce the Lambda resource policy on an AWS_PROXY integration.
AWS: a REST API AWS_PROXY (Lambda proxy) integration may invoke the function only if the function's resource policy holds a lambda:InvokeFunction statement for principal apigateway.amazonaws.com with the method's source ARN. Without that statement the integration invoke is denied: the method answers HTTP 500 with body {"message": "Internal server error"} and the execution log records the denied Lambda invocation (AWS documents 'Invalid permissions on Lambda function'/'not authorized to perform: lambda:InvokeFunction'), and the function never executes. Adding the permission afterwards makes the identical request succeed with the handler's own 200 body. The fixed requirement is the 500 refusal and a non-executing integration; the function name, API id and source ARN are values of the request.
Emulator: the same request is served 200 with the handler's body ({"marker": "..."}) and an execution log ending 'Integration type: AWS_PROXY / Successfully completed execution / Method completed with status: 200' — i.e. the integration invokes the function with no authorization check at all, so the negative case is indistinguishable from the positive control.
Why: the Lambda resource policy is the only thing standing between a public API stage and a function. An emulator that invokes regardless lets a client ship an API whose integration permissions were never wired (or were wired to the wrong source ARN) and discover it only in production; least-privilege tests and permission-boundary tests all pass vacuously.
Fix: the API Gateway integration-invoke path must evaluate the target function's resource policy (principal, action, SourceArn) before invoking and return the documented 500 refusal with the denied-invocation execution-log line when no statement matches.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected.

Cause 2 of 3: a REST stage's access logs and execution logs never reach the configured CloudWatch Logs destination.
AWS: with an account-level CloudWatch role installed (UpdateAccount cloudwatchRoleArn) and a stage carrying accessLogSettings {destinationArn, format} and/or methodSettings loggingLevel/dataTrace, traffic through the deployed stage URL produces, within seconds, one access-log record per request in the destination log group rendered by the stage's $context format (and, for execution logging, the per-request 'Execution log for request <id>' trace at the configured level), each stage writing only into its own destination; a stage with no account CloudWatch role, or whose destination the role may not create, produces no group and no records while still serving traffic.
Emulator: these checks never complete inside the suite window — the poll for a 200 from the deployed stage URL and/or for the access/execution-log records in the destination group never settles, so neither the positive delivery nor the negative 'nothing delivered' assertions can be graded.
Why: access and execution logs are how a client proves what its API served and debugs a failing integration; a stage that serves nothing observable (or serves nothing at all) makes every logging configuration untestable and hides per-stage leakage between destinations.
Fix: the API Gateway stage data-plane — the deployed-stage invoke endpoint and the access-log/execution-log emitter that renders the stage's format and writes to the accessLogSettings destination through the account CloudWatch role. Open issues 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md, 20261104T044826Z-apigateway-dynamodb-lambda-logs-api-gateway-stage-logging-checks-never-complete-and.md and 20261102T090835Z-apigateway-dynamodb-lambda-logs-api-gateway-rest-proxy-integrations-never-reach-a.md describe this area for other checks and do not claim these.
Traces: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage, apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format, apigateway__dynamodb__lambda__logs.apigw-execution-logs-delivered-to-cloudwatch, apigateway__dynamodb__lambda__logs.apigw-http-api-access-logs-json-format, apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative.

Cause 3 of 3: a CLI-driven create→integrate→deploy→invoke roundtrip never reaches a served response.
AWS: the documented CLI sequence (create-rest-api, create-resource, put-method, put-integration of type AWS_PROXY, create-deployment --stage-name, add-permission) leaves a stage whose invoke URL immediately returns the function's response (200 with its JSON body), with the handler's printed marker visible in /aws/lambda/<fn>; a request to a stage that was never deployed is refused with HTTP 403 {"message":"Forbidden"}.
Emulator: the three CLI roundtrip checks never complete inside the window — the poll for the stage URL's response (and for the CloudWatch-visible marker proving the function ran) never settles, so neither the happy path nor the missing-deployment refusal is graded.
Why: this is the canonical first-run path for an API; a client following it gets no response from the URL the emulator handed back, so nothing built on a deployed stage can be exercised.
Fix: the same deployed-stage invoke path as cause 2, plus the deployment/stage state the invoke URL resolves against (including the not-deployed 403 refusal).
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip, apigateway__dynamodb__lambda__logs.cli-apigateway-lambda-integration-setup-and-test, apigateway__dynamodb__lambda__logs.cli-apigateway-missing-deployment-stage-negative.

## Evidence
Cause 1 — expected: 500 for the AWS_PROXY method whose Lambda has no apigateway invoke permission. Actual: AssertionError: expected 500 without permission, got {'status': 200, 'body': '{"marker": "sim2realpermcheckok"}', 'headers': {'Content-Type': 'application/json'}, ... 'log': 'Execution log for request test-invoke-request ... Integration type: AWS_PROXY ... Successfully completed execution ... Method completed with status: 200'} (check line 85).

Causes 2 and 3 — expected: a 200 from the deployed stage URL and the stage's access/execution-log records in the configured destination group within seconds. Actual: every one of these eight checks ends with 'Error: Check timed out after 60.0s' while polling the stage URL / the destination log group.

## Steps to Reproduce
1. Cause 1: create a python3.12 Lambda and a REST API with a GET /echo AWS_PROXY integration to it, do NOT call lambda add-permission, then invoke the method (aws apigateway test-invoke-method / the deployed stage URL) — a 200 with the handler body is returned instead of 500; add the permission and the identical call behaves the same.
2. Cause 2: install an account CloudWatch role (apigateway update-account cloudwatchRoleArn), create two stages of one API each with accessLogSettings destinationArn pointing at its own log group plus a $context format, send traffic to stage A's URL and poll both groups — no access-log record appears (and the stage URL does not answer).
3. Cause 3: run the CLI sequence create-rest-api → create-resource → put-method → put-integration (AWS_PROXY) → create-deployment --stage-name s → add-permission, then GET https://<api-id>.execute-api.<region>.<host>/s/echo?token=x and poll /aws/lambda/<fn> for 'MARKER:x' — neither the response nor the marker ever arrives; a request to an undeployed stage does not produce the 403 refusal either.
