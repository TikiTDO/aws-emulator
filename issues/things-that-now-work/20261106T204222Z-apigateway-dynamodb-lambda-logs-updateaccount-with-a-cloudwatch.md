Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative|_invoke_until_log_events#31|Timeout|time.sleep(interval_s)
Issue-Commit: 09448fb928879ffb83c121d01adc49e65396319c

# apigateway__dynamodb__lambda__logs: UpdateAccount with a CloudWatch role that does not trust apigateway.amazonaws.com never reaches a terminal answer — iam-role-missing-apigateway-trust-negative hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `09448fb928879ffb83c121d01adc49e65396319c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccount

## Description
One net-new root cause in the apigateway__dynamodb__lambda__logs group (1 of 1).

Cause 1 of 1: the account-CloudWatch-role TRUST-POLICY boundary on UpdateAccount, and its positive control, reach no verdict at all — the check blocks and is cut off at the harness cap with no observation recorded. Failing check: apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative [mismatch], detail `Check timed out after 60.0s`.

AWS: every call in this sequence is a synchronous control-plane call (plus one data-plane invoke) and each one answers on the first attempt.
  * Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json): GetAccount GET /account -> Account (200); UpdateAccount PATCH /account -> Account (200), input `patchOperations` (ListOfPatchOperation), modeled errors BadRequestException (httpError 400), ConflictException (409), LimitExceededException (429), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). The Account output carries the `cloudwatchRoleArn` member.
  * UpdateAccount patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: <roleArn>}] naming an IAM role whose trust policy grants sts:AssumeRole to lambda.amazonaws.com and NOT to apigateway.amazonaws.com is REFUSED: HTTP 400, Error.Code `BadRequestException`, Error.Message `The role ARN does not have required permissions configured. Please grant trust permission for API Gateway and add the required role policy.` That message is ENTIRELY FIXED TEXT for this error class — it quotes back neither the role ARN nor the role name, so no part of it is taken from the request or from state. (The message wording is not modeled: recorded behavior only, and the emulator's own handler already declares the same sentence as `ROLE_WITHOUT_REQUIRED_PERMISSIONS`.)
  * The refusal is terminal, not transient: it is not a TooManyRequestsException/ThrottlingException, so a client retrying only on throttling sees it on the first call.
  * The refused call has no effect: an independent GetAccount still reports the `cloudwatchRoleArn` the account held before the attempt (absent if it held none).
  * UpdateAccount then ACCEPTS, on the first call, a role that does trust apigateway.amazonaws.com and has arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs attached (HTTP 200 Account body whose cloudwatchRoleArn is that role), GetAccount reports it, and a deployed stage with /*/*/logging/loglevel=INFO then delivers execution log events into API-Gateway-Execution-Logs_<apiId>/<stageName>.

Emulator: the check produces NO answer. The only replay detail is `Check timed out after 60.0s` — no traceback and no assertion, so neither the refusal nor the positive control is reached as an observation. The handler for this operation is NOT a 501 stub: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_account.rs implements the patch walk and does ask whether the role is assumable by apigateway.amazonaws.com before storing it, so a request that reaches it has a path to the correct 400. What does not happen is TERMINATION: the leg that asks IAM about the role is awaited inside the request, and the check's bounded retry loop (which retries only on throttling codes and otherwise returns the first terminal ClientError) consumes the whole budget, which only happens if that call never answers or answers with a transient code. Hypothesis (neither the model nor the replay detail states the mechanism, which carries only the timeout): the in-emulator role-assumability lookup that UpdateAccount awaits does not complete, so the PATCH /account request never produces a status line. Related open issues describe the SAME group hanging on neighbouring legs but carry different replay keys and so do not cover this one: 20261031T010204Z-apigateway-dynamodb-lambda-logs-the-account-cloudwatch-role.md (the UpdateStage precondition when the account role is unset) and 20261024T043741Z-apigateway-dynamodb-lambda-logs-three-logging-account-wiring.md (access/execution logging and the CFN-deployed Account resource).

Why: a client cannot tell any of the three outcomes AWS defines here apart. The misconfiguration it is supposed to catch at configuration time — handing API Gateway a CloudWatch role it can never assume — is neither refused nor accepted, it simply never answers, so a deployment script that sets the account role blocks instead of either succeeding or reporting a 400 it can act on. Because no call returns, the no-op guarantee (a refused UpdateAccount leaving cloudwatchRoleArn exactly as it was) is also unobservable, and any later fix to the refusal itself cannot be confirmed from this check.

Fix: the apigateway service's UpdateAccount handler and the role-assumability resolution it awaits (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_account.rs and the shared iam_role helper it calls) must always produce a response: the trust-policy question has to resolve to an answer — or to "not refused" when it cannot be answered — within the request, so PATCH /account returns either 400 BadRequestException with the fixed sentence above or a 200 Account body. GetAccount must keep reporting the stored role unchanged across a refused update.

Traces: apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative.

## Evidence
Replay of apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative [mismatch]:

  Error: Check timed out after 60.0s

(That is the entire detail the replay carries — no traceback, no assertion, so no step of the sequence produced an observation.)

Expected (AWS), in order:
  1. GetAccount -> HTTP 200 Account body; note the current cloudwatchRoleArn.
  2. IAM CreateRole with a trust policy granting sts:AssumeRole to lambda.amazonaws.com only, plus AttachRolePolicy arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs.
  3. UpdateAccount patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: <badTrustRoleArn>}]
       -> HTTP 400, Error.Code = BadRequestException,
          Error.Message = "The role ARN does not have required permissions configured. Please grant trust permission for API Gateway and add the required role policy."
          (entirely fixed text for this error class; no request- or state-derived parts, and NOT a throttling/transient code, so it arrives on the first call).
  4. GetAccount -> cloudwatchRoleArn still exactly the value read in step 1 (the refused patch left no trace).
  5. IAM CreateRole with a trust policy granting sts:AssumeRole to apigateway.amazonaws.com, same managed policy attached; UpdateAccount replacing /cloudwatchRoleArn with it -> HTTP 200 on the first call, and GetAccount reports that role.
  6. A deployed REST stage with a MOCK integration and /*/*/logging/loglevel=INFO answers its execute-api GET, and API-Gateway-Execution-Logs_<apiId>/<stageName> accumulates at least one execution log event.

Actual (emulator):
  None of the above is observed. The check is cut off at the 60 s cap with no status line and no error code recorded for the UpdateAccount call carrying the bad-trust role, so the BadRequestException refusal, the no-op guarantee and the positive-control log delivery are all ungraded.

## Steps to Reproduce
1. Boot the emulator and replay apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative; it is cut off at the 60 s cap with no assertion reached.
2. By hand, with the AWS CLI pointed at the emulator, TIMING each call:
   a. aws apigateway get-account — note cloudwatchRoleArn (it must answer promptly).
   b. aws iam create-role --role-name badtrust --assume-role-policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}' ; aws iam attach-role-policy --role-name badtrust --policy-arn arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs
   c. aws apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=<badtrust role ARN>
      Expected: HTTP 400 BadRequestException "The role ARN does not have required permissions configured. Please grant trust permission for API Gateway and add the required role policy." — returned on the FIRST call.
      Observed: the call does not reach a terminal answer; this is the step that consumes the budget.
   d. aws apigateway get-account — cloudwatchRoleArn must be unchanged from (a).
   e. aws iam create-role --role-name goodtrust with the same document but Principal.Service = apigateway.amazonaws.com, attach the same managed policy, then aws apigateway update-account --patch-operations op=replace,path=/cloudwatchRoleArn,value=<goodtrust role ARN> — must return HTTP 200 on the first call, and get-account must report that role.
   f. Create a REST API with a /echo GET MOCK integration, create-deployment --stage-name test, update-stage --patch-operations op=replace,path=/*/*/logging/loglevel,value=INFO, GET the stage URL, then aws logs describe-log-streams / get-log-events on API-Gateway-Execution-Logs_<apiId>/test — at least one event must be present.
