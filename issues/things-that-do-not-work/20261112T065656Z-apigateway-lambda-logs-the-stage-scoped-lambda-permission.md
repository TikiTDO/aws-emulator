Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged
Issue-Key: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged|run#36|Timeout|assert first_allowed_200_ts is None and time.time() - t_loop < 60, (
Issue-Commit: dedd6f8a44979b422a25c34412c44daa0d34e6e0

# apigateway__lambda__logs: the stage-scoped Lambda-permission differential never reaches a verdict — the two-stage invoke/execution-log trajectory hangs to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `dedd6f8a44979b422a25c34412c44daa0d34e6e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
Cause 1 of 1: a REST API whose Lambda grant is scoped by SourceArn to one stage never produces the observable differential AWS produces, and the check reaches no verdict at all — it is cut off by the harness budget instead of returning an answer.

AWS: recorded behavior only (there is no single modeled operation for the execute-api data plane; the CloudWatch Logs reads use logs#FilterLogEvents / logs#DescribeLogGroups, and the grant is lambda#AddPermission with Principal=apigateway.amazonaws.com and SourceArn=arn:aws:execute-api:<region>:<account>:<apiId>/<stage>/*/*). On AWS a grant naming one stage admits only that stage: a request to the granted stage is served 200 by the integration function and its marker appears in that function's log group /aws/lambda/<functionName>; a same-burst request to any other stage of the same deployment is answered 500 with the gateway's own body {"message": "Internal server error"}, the function is never invoked for it, and the stage's execution log group API-Gateway-Execution-Logs_<apiId>/<stage> carries an ERROR line, prefixed with the request's own id as `(<x-amzn-RequestId>)`, whose text is the fixed string `Execution failed due to configuration error: Invalid permissions on Lambda function`. Adding a second grant for the previously refused stage makes that same stage serve 200 and reach the function. Fixed text in that message: `Execution failed due to configuration error: Invalid permissions on Lambda function`; the only variable parts are the request id in the line prefix and the stage/api/function names, all taken from the request and the account's state.

Emulator: the check never completes. The suite reports `Error: Check timed out after 60.0s` with no assertion reached, so no leg of the differential (granted-stage 200 + marker in /aws/lambda/<fn>, refused-stage 500 + invalid-permissions ERROR line in API-Gateway-Execution-Logs_<apiId>/denied) ever became observable inside the harness budget; the check's own 180 s barrier deadline is never even reached. Hypothesis: the stage data plane and/or the API Gateway execution log group never answer, which is the same shape as the already-open apigateway__lambda__logs issues 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md, 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md and 20261110T224806Z-apigateway-lambda-logs-a-rest-stage-carrying-accesslogsettings.md — none of those carries this check's replay key, so this cause is filed separately and those issues are named here only as neighbours.

Why: a client that scopes a Lambda resource policy per stage cannot verify the scope at all. Nothing distinguishes 'the other stage was correctly refused' from 'the emulator never served or logged anything', so an over-permissive deployment (every stage reaching the function) and a correctly scoped one look identical, and the authoritative evidence AWS gives — the invalid-permissions ERROR line in the stage execution log — is absent.

Fix: the API Gateway execute-api stage handler that resolves an AWS_PROXY integration and evaluates the target function's resource policy against the per-stage source ARN, plus the execution-logging path that writes API-Gateway-Execution-Logs_<apiId>/<stage> records at the stage's configured loglevel; and whatever call in that path does not return, since the trajectory currently does not terminate.

Traces: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged.

## Evidence
Failing check: [mismatch] apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged
Replay detail (verbatim): `Error: Check timed out after 60.0s`

Expected (AWS): granted stage -> 200 and the request marker in /aws/lambda/<functionName>; other stage of the same deployment -> HTTP 500 with body {"message": "Internal server error"}, no function invocation, and an ERROR record in API-Gateway-Execution-Logs_<apiId>/<stage> beginning `(<x-amzn-RequestId>)` and containing `Execution failed due to configuration error: Invalid permissions on Lambda function`; after lambda:AddPermission for the second stage, that stage serves 200 and its marker reaches the function log group.

Actual (emulator): no verdict — the check is terminated by the 60 s harness cap before any of those observations is made. No assertion message is produced, i.e. the trajectory never returns a terminal answer.

## Steps to Reproduce
1. Create a REST API (REGIONAL) with resource /echo, method ANY (authorizationType NONE) and an AWS_PROXY integration pointing at a python Lambda function.
2. Create one deployment and two stages, `allowed` and `denied`, and set /*/*/logging/loglevel = ERROR on both.
3. lambda AddPermission on the function: Principal=apigateway.amazonaws.com, Action=lambda:InvokeFunction, SourceArn=arn:aws:execute-api:<region>:<account>:<apiId>/allowed/*/*.
4. In one burst, GET https://<apiId>.execute-api.<region>.<host>/allowed/echo?marker=<m1> and .../denied/echo?marker=<m2>.
5. Expect 200 on /allowed with <m1> appearing in /aws/lambda/<functionName>, and 500 with body {"message":"Internal server error"} on /denied with an ERROR line `(<requestId>) ... Execution failed due to configuration error: Invalid permissions on Lambda function` in API-Gateway-Execution-Logs_<apiId>/denied.
6. Then AddPermission for .../denied/*/* and expect /denied to serve 200 and reach the function log group.
Observed: the run never reaches step 5 — it is cut off at the 60 s harness cap.
