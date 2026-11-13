Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative|_wait_for_200#20|Timeout|time.sleep(5)
Issue-Commit: 5c0f2e9076960c0c5ec367c612fbea7052ab391b

# apigateway__dynamodb__lambda__logs: the account-CloudWatch-role precondition check now reaches no verdict at all — apigw-missing-cloudwatch-role-arn-no-logs-negative hangs to the 60 s cap instead of failing its UpdateStage assertion (1 cause)

> _Note: this issue was identified against the code base at commit `5c0f2e9076960c0c5ec367c612fbea7052ab391b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One net-new root cause in the apigateway__dynamodb__lambda__logs group (1 of 1).

Cause 1 of 1: the account-level CloudWatch Logs role precondition for stage execution logging, and its positive control, can no longer be observed at all — the check blocks and reaches no verdict within the harness cap. Failing check: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative [mismatch], `Check timed out after 60.0s`.

AWS: the whole sequence this check drives is a sequence of synchronous control-plane calls plus one data-plane invocation, every one of which answers promptly on AWS.
  * IAM CreateRole/AttachRolePolicy, Lambda CreateFunction (reaching State=Active), API Gateway CreateRestApi/CreateResource/PutMethod/PutIntegration (AWS_PROXY), Lambda AddPermission and CreateDeployment all answer on the first call.
  * With the account singleton's cloudwatchRoleArn cleared (UpdateAccount patchOperations [{op: replace, path: /cloudwatchRoleArn, value: ""}], then GetAccount reporting it absent/empty), an UpdateStage patch [{op: replace, path: /*/*/logging/loglevel, value: INFO}] is REFUSED with HTTP 400, error code BadRequestException, message `CloudWatch Logs role ARN must be set in account settings to enable logging` — that text is entirely fixed for this error class and carries no values from the request or from state — and an independent GetStage still reports methodSettings["*/*"].loggingLevel absent/OFF.
  * UpdateAccount then accepts a well-formed cloudwatchRoleArn (a role trusting apigateway.amazonaws.com with arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs attached) on the FIRST call, the same UpdateStage patch is then accepted (HTTP 200 Stage document), GET on the deployed stage's execute-api URL answers HTTP 200 with the echo integration's body, and the group API-Gateway-Execution-Logs_<apiId>/<stageName> comes into existence and accumulates execution log events readable with FilterLogEvents.
  Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice): UpdateStage PATCH /restapis/{restApiId}/stages/{stageName} -> Stage, errors include BadRequestException (httpError 400); UpdateAccount PATCH /account -> Account and GetAccount GET /account both carry the cloudwatchRoleArn member. The precondition itself and its message wording are not modeled — recorded behavior only for the message text, which the recording shows as the fixed sentence above.

Emulator: the check produces NO answer at all. The replay verdict is `mismatch` with the single detail `Check timed out after 60.0s`: no assertion fires, so the refusal/acceptance of UpdateStage is not even reached as an observation. This is a CHANGED failure mode for this same check id: it previously failed fast on `assert raised, "expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn"` (the open issue 20261030T163201Z-apigateway-dynamodb-lambda-logs-updatestage-enables-stage-execution-logging-with-no-account.md carries exactly that replay key), and the replay report no longer reports this check as owned by it, so the key it fails under today is a different one. Hypothesis (not stated by the model and not visible in the replay detail, which carries only the timeout): the block is in one of the check's bounded retry loops that each spin for the whole cap — `_set_account_cw_role` retries UpdateAccount for 60 s when the emulator refuses a well-formed cloudwatchRoleArn, and `_wait_for_200` retries the deployed stage's execute-api GET for 60 s when the invoke never answers 200. Both of those symptoms are described for sibling checks by the open issues 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md (deployed-stage invoke / account logging role / log delivery, 9 causes) and 20261030T171805Z-apigateway-updatestage-accepts-enabling-stage-execution-logging-while-the.md (the same precondition in the apigateway group) — neither covers this check's current replay key, which is why it is filed here.

Why: a client cannot distinguish any of the three outcomes AWS defines here. The configuration-time boundary (refused with BadRequestException) is not observable, the positive control (role set, stage serves 200, execution logs delivered) is not observable, and the operator gets no answer at all rather than a wrong one — every call in the sequence must terminate, so a tool that scripts "clear the role, expect a refusal, set the role, expect delivery" hangs instead of either passing or reporting a defect. It also means a fix to the UpdateStage precondition cannot be confirmed: the check can no longer reach the assertion that would prove it.

Fix: the apigateway service's UpdateStage handler validation for method-setting logging paths (/*/*/logging/loglevel and the per-method equivalents) must read the apigateway account singleton's cloudwatchRoleArn and refuse with 400 BadRequestException and the fixed message above while it is unset, leaving methodSettings unmodified; and the calls this check's sequence makes before and after it must each answer on the first attempt rather than being retried to a deadline — the UpdateAccount handler accepting a well-formed cloudwatchRoleArn, and the execute-api data plane serving the deployed stage's AWS_PROXY GET together with creation of and delivery into API-Gateway-Execution-Logs_<apiId>/<stageName>.

Traces: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative

## Evidence
Replay of apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative [mismatch]:

  Error: Check timed out after 60.0s

(That is the whole detail the replay carries — no traceback, no assertion, so no step of the sequence produced an observation.)

Expected (AWS), in order:
  1. UpdateAccount patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: ""}] -> HTTP 200; GetAccount reports cloudwatchRoleArn absent/empty.
  2. UpdateStage patchOperations=[{op: replace, path: /*/*/logging/loglevel, value: INFO}]
       -> HTTP 400, Error.Code = BadRequestException,
          Error.Message = "CloudWatch Logs role ARN must be set in account settings to enable logging"
          (entirely fixed text for this error class; no request- or state-derived parts).
  3. GetStage -> methodSettings["*/*"].loggingLevel absent / OFF (the refused patch left no trace).
  4. UpdateAccount patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: <role trusting apigateway.amazonaws.com with AmazonAPIGatewayPushToCloudWatchLogs>}] -> HTTP 200 on the first call.
  5. The same UpdateStage patch -> HTTP 200 Stage document with methodSettings["*/*"].loggingLevel = INFO.
  6. GET <execute-api stage URL>/echo -> HTTP 200 with the echo integration's JSON body.
  7. FilterLogEvents on API-Gateway-Execution-Logs_<apiId>/<stageName> -> at least one delivered execution log event.

Actual (emulator):
  The check never reaches any of those assertions; it is cut off at the 60 s harness cap with no observation recorded. Previously the same check id failed at step 2 with
    AssertionError: expected UpdateStage to reject enabling logging with no account cloudWatchRoleArn
  so the divergence has moved from a wrong answer to no answer at all.

## Steps to Reproduce
1. Boot the emulator and replay apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative; it is cut off at the 60 s cap with no assertion reached.
2. By hand, with the AWS CLI pointed at the emulator, timing each call:
   a. Create an IAM role trusting lambda.amazonaws.com with AWSLambdaBasicExecutionRole attached; lambda create-function (python3.12, handler index.handler, a zip whose index.py returns {'statusCode': 200, 'headers': {'Content-Type': 'application/json'}, 'body': '{"ok": true}'}); wait for Configuration.State = Active.
   b. apigateway create-rest-api; create-resource pathPart=echo under /; put-method GET authorizationType=NONE; put-integration --type AWS_PROXY --integration-http-method POST --uri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations; lambda add-permission Principal=apigateway.amazonaws.com SourceArn=arn:aws:execute-api:<region>:<acct>:<apiId>/*/*; create-deployment --stage-name prod.
   c. apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value='' then get-account; confirm cloudwatchRoleArn is empty/absent.
   d. apigateway update-stage --rest-api-id <id> --stage-name prod --patch-operations op=replace,path=/*/*/logging/loglevel,value=INFO
      Expected: HTTP 400 BadRequestException "CloudWatch Logs role ARN must be set in account settings to enable logging".
   e. apigateway get-stage --rest-api-id <id> --stage-name prod; AWS leaves methodSettings["*/*"].loggingLevel OFF/absent.
   f. Create an IAM role trusting apigateway.amazonaws.com with arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs attached and apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=<roleArn>. It must succeed on the FIRST call (the check otherwise retries this for 60 s and consumes the whole budget).
   g. Re-issue step (d); it must now return HTTP 200 with loggingLevel INFO.
   h. curl https://<apiId>.execute-api.<region>.<host>/prod/echo — it must answer HTTP 200 (the check otherwise retries this for 60 s).
   i. logs filter-log-events --log-group-name API-Gateway-Execution-Logs_<apiId>/prod — at least one event must be present.
   Whichever of (f), (h) or (i) does not answer is the step that consumes the cap and leaves the check without a verdict.
