Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative|run#14|AssertionError|assert _poll_log_group_created(ctx, missing_group_name, timeout_s=90) is False, (
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|run#38|AssertionError|assert still_missing, f"log group {bad_log_group} unexpectedly came into existence"
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative|run#19|AssertionError|assert bad_messages == [], (
Issue-Commit: c631c395726115bfaae59953a7633393b63383b8

# apigateway__dynamodb__lambda__logs: 2 net-new causes — API Gateway CloudWatch log delivery ignores the account cloudwatchRoleArn's IAM policy, so an explicit Deny neither blocks access-log log-group creation nor execution-log writes

> _Note: this issue was identified against the code base at commit `c631c395726115bfaae59953a7633393b63383b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
API Gateway writes both access logs and execution logs to CloudWatch Logs by assuming the account-level role named by Account.cloudwatchRoleArn (Smithy: com.amazonaws.apigateway#Account member cloudwatchRoleArn — "The ARN of an Amazon CloudWatch role for the current Account"; com.amazonaws.apigateway#AccessLogSettings member destinationArn — "The Amazon Resource Name (ARN) of the CloudWatch Logs log group ... to receive access logs"). Every CloudWatch Logs call API Gateway makes for a stage is therefore an IAM-authorized call made by that role, and an explicit Deny in the role's identity policy for a specific log-group ARN must stop it. The emulator performs these log-delivery calls unauthorized: it never evaluates the assumed role's policy, so a Deny is a no-op. Two distinct delivery paths are affected and each must be closed before this issue is submitted.

Cause 1 of 2: the access-log destination log group is auto-created even when the account CloudWatch role is explicitly denied logs:CreateLogGroup on that exact log-group ARN.
  AWS: when a stage's accessLogSettings.destinationArn names a CloudWatch Logs log group that does not exist, API Gateway attempts to create it as the assumed cloudwatchRoleArn role. If that role's identity policy carries an explicit Deny for logs:CreateLogGroup scoped to that log-group ARN (and its ":*" form), the create is refused by IAM, the log group never comes into existence, and the failure is silent on the data plane — the stage keeps answering 200 to invocations and still returns an x-amzn-RequestId header. A role without that Deny does get its destination group created. (No API Gateway operation models this; authorization of the delivery call is AWS documented behavior plus recorded behavior only: the recording shows the denied group absent and the undenied group present.)
  Emulator: after UpdateStage sets /accessLogSettings/destinationArn to the denied ARN and the stage is invoked, the destination log group is created anyway — DescribeLogGroups(logGroupNamePrefix=<denied name>) returns it. The replay fails with `AssertionError: log group <name> was created despite an explicit Deny on logs:CreateLogGroup for that exact destination ARN` and, from the CLI variant, `AssertionError: log group <name> unexpectedly came into existence`. (`<name>` here is the per-run generated group name — only the fact of its existence is the requirement.) Log-group creation is evidently unconditional on configuring accessLogSettings rather than performed as the account role.
  Why: a client relying on AWS's behavior uses the CloudWatch role's policy as the boundary on where API Gateway may write. A permission-boundary test that asserts a denied destination is never materialized passes against AWS and fails here; worse, an emulated deployment reports that a least-privilege role is sufficient when on AWS the same role silently loses all access logs for that stage.
  Fix: the stage access-log delivery path — the component that provisions/creates the accessLogSettings destination log group — must make that CreateLogGroup as the account cloudwatchRoleArn principal and run it through the IAM policy evaluator (explicit Deny wins), skipping creation and delivery when refused, while keeping the invoke answering 200.
  Traces: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative (also fails its positive control's sibling expectations), apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected.

Cause 2 of 2: execution logs are written to a stage's API-Gateway-Execution-Logs_<api-id>/<stage> group even when the account CloudWatch role is explicitly denied logs:CreateLogStream and logs:PutLogEvents on that exact log-group ARN.
  AWS: with /*/*/logging/loglevel = INFO, API Gateway writes per-request execution log events (the "Execution log for request <request-id>", "Starting execution for request: <request-id>", "HTTP Method: <method>, Resource Path: <path>" family) into API-Gateway-Execution-Logs_<api-id>/<stage> as the assumed cloudwatchRoleArn role. An explicit Deny for logs:CreateLogStream/logs:PutLogEvents scoped to that log group's ARN makes those writes fail in IAM, so the group stays empty, while a sibling stage of the same API whose group is outside the Deny receives its events (including its own request id). The invocation itself still returns 200 with an x-amzn-RequestId header. Recorded behavior only for the enforcement point; the message family above is API Gateway's fixed execution-log text with the request id, HTTP method and resource path taken from the request.
  Emulator: the denied group receives execution log events. The replay fails with `AssertionError: execution logs appeared in API-Gateway-Execution-Logs_<api-id>/<stage> despite an explicit Deny on PutLogEvents/CreateLogStream for that exact log group ARN: ['Execution log for request <request-id>', 'Starting execution for request: <request-id>', 'HTTP Method: GET, Resource Path: /ping']` — i.e. the emulator emits the right execution-log text but emits it unauthorized (the api id, stage, request id are per-run values; the divergence is that any event is delivered at all).
  Why: the same boundary problem in the opposite direction from the open delivery issues — a client validating that a scoped-down CloudWatch role suppresses execution logging for one stage gets the wrong answer, and operators cannot test least-privilege logging roles against the emulator.
  Fix: the stage execution-logging path — the component that opens the log stream and puts execution events for a stage — must perform CreateLogStream/PutLogEvents as the account cloudwatchRoleArn principal through the IAM policy evaluator and drop delivery silently when denied, leaving the request's own 200 response and x-amzn-RequestId untouched.
  Traces: apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative.

Related but not covering: the open issues 20261026T072730Z-apigateway-dynamodb-lambda-logs-apigateway-a-stage-with-accesslogsettings-custom-context-json.md, 20261105T074825Z-apigateway-dynamodb-lambda-logs-a-stage-whose-accesslogsettings-point-at-a-pre.md, 20261107T212447Z-apigateway-dynamodb-lambda-logs-rest-stage-access-logging-never-converges-enabling-accesslogsettings.md and 20261108T120220Z-apigateway-dynamodb-lambda-logs-a-rest-stage-with.md all describe log delivery NOT happening where AWS delivers; these two causes are the inverse — delivery happening where AWS's IAM evaluation forbids it — so none of them carry these checks.

## Evidence
Cause 1 (access-log destination group created despite Deny):
  Setup: IAM role trusted by apigateway.amazonaws.com with a blanket Allow on logs:CreateLogGroup/CreateLogStream/PutLogEvents/Describe*/Get*/FilterLogEvents on "*", plus an explicit Deny statement on logs:CreateLogGroup for Resource [<group-arn>, <group-arn>:*]; set as the account cloudwatchRoleArn via UpdateAccount; stage's /accessLogSettings/destinationArn set to <group-arn>.
  Expected (AWS): stage invoke -> 200 with x-amzn-RequestId; DescribeLogGroups(logGroupNamePrefix=<group>) never lists <group> (polled 90 s); a second role with no Deny and a fresh destination DOES get its group created.
  Actual (emulator): <group> is listed — AssertionError: "log group sim2real-behav-never-created-457131aaf549 was created despite an explicit Deny on logs:CreateLogGroup for that exact destination ARN". CLI variant: AssertionError: "log group sim2real-behav-accesslog-bad-c28493f44fbb unexpectedly came into existence". (Both names are per-run generated; the existence is the divergence.)

Cause 2 (execution logs written despite Deny):
  Setup: same role shape, explicit Deny on logs:CreateLogStream + logs:PutLogEvents for the pre-created group API-Gateway-Execution-Logs_<api-id>/<stage> and its ":*" form; stage patched /*/*/logging/loglevel = INFO; stage invoked once.
  Expected (AWS): invoke -> 200 with x-amzn-RequestId; the denied group yields NO log events (polled 90 s); the sibling stage outside the Deny yields execution events containing its own request id.
  Actual (emulator): the denied group holds execution events — AssertionError: "execution logs appeared in API-Gateway-Execution-Logs_eocd2yo7iw/sim2realdenyeocd2yo7 despite an explicit Deny on PutLogEvents/CreateLogStream for that exact log group ARN: ['Execution log for request ae1ee0e5-0df9-477a-89e5-b3198b378523', 'Starting execution for request: ae1ee0e5-0df9-477a-89e5-b3198b378523', 'HTTP Method: GET, Resource Path: /ping']". The api id, stage name and request id are per-run values.

Common diagnosis: no IAM authorization is applied to the CloudWatch Logs calls API Gateway makes on behalf of Account.cloudwatchRoleArn; the role is only validated at UpdateAccount time, never consulted at delivery time.

## Steps to Reproduce
Cause 1:
1. sts:GetCallerIdentity to get the account id; pick a log-group name that does not exist and build <group-arn> = arn:aws:logs:<region>:<account>:log-group:<name>.
2. iam:CreateRole with a trust policy allowing sts:AssumeRole for Service apigateway.amazonaws.com; iam:PutRolePolicy with Allow on logs:CreateLogGroup/CreateLogStream/PutLogEvents/DescribeLogGroups/DescribeLogStreams/GetLogEvents/FilterLogEvents on "*" AND an explicit Deny on logs:CreateLogGroup for [<group-arn>, <group-arn>:*].
3. apigateway:UpdateAccount patchOperations replace /cloudwatchRoleArn with that role ARN.
4. Create a REST API with a MOCK GET /ping integration returning 200; CreateDeployment with a stage; UpdateStage replace /accessLogSettings/destinationArn = <group-arn> and /accessLogSettings/format = '{"requestId":"$context.requestId"}'.
5. GET https://<api-id>.execute-api.<region>.<host>/<stage>/ping — expect 200 + x-amzn-RequestId.
6. Poll logs:DescribeLogGroups(logGroupNamePrefix=<name>) for 90 s. AWS: never present. Emulator: present.
7. Control: repeat with a role carrying no Deny and a fresh destination name — the group must be created.

Cause 2:
1. Create a REST API with the MOCK GET /ping integration; choose bad_stage and good_stage names.
2. logs:CreateLogGroup for API-Gateway-Execution-Logs_<api-id>/<bad_stage>; build its ARN.
3. Create the apigateway-trusted role with the blanket logs Allow plus an explicit Deny on logs:CreateLogStream and logs:PutLogEvents for [<that-arn>, <that-arn>:*]; set it as cloudwatchRoleArn via UpdateAccount.
4. CreateDeployment stageName=bad_stage; UpdateStage replace /*/*/logging/loglevel = INFO; GET the stage's /ping (expect 200 + x-amzn-RequestId).
5. Poll logs:FilterLogEvents/GetLogEvents on API-Gateway-Execution-Logs_<api-id>/<bad_stage> for 90 s. AWS: no events. Emulator: the three execution-log lines appear.
6. Control: deploy good_stage with loglevel INFO and invoke it — its own group must receive events containing that invocation's request id.
