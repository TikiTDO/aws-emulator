Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group
Issue-Key: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group|_log_events_containing#12|Timeout|return out
Issue-Commit: 677ec28d2ee9442908109a9e8e0002b68ccccfcc

# apigateway__lambda__logs: a REST stage carrying accessLogSettings delivers no access-log record for the requests it serves, so the check reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `677ec28d2ee9442908109a9e8e0002b68ccccfcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a deployed REST stage whose accessLogSettings name a CloudWatch Logs destination and a custom $context JSON format never delivers an access-log record for the requests the stage serves, so none of the $context variables in the configured format is ever resolved and observable. Failing check id: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group.

  AWS: Smithy (apigateway): Stage.accessLogSettings -> AccessLogSettings{destinationArn, format}; the model documents destinationArn as "The Amazon Resource Name (ARN) of the CloudWatch Logs log group or Kinesis Data Firehose delivery stream to receive access logs" and format as "A single line format of the access logs of data, as specified by selected $context variables. The format must include at least $context.requestId". The settings are applied through CreateStage/UpdateStage (and UpdateStage refuses the patch unless the account-level cloudwatchRoleArn is configured, which this scenario sets first). Recorded behavior only for the delivery itself: once such a stage serves a request, exactly one access-log record per served request is written to the named log group, with every $context variable in the configured format resolved from THAT request and the stage serving it — $context.httpMethod = the request verb, $context.status = the status actually returned, $context.stage = the stage name, $context.resourcePath = the un-prefixed resource path (e.g. "/ping"), $context.path = the STAGE-QUALIFIED request path (e.g. "/<stage>/ping", and therefore different from resourcePath), $context.requestId = the non-empty id minted for that request. These are template slots filled from the request and state; no literal value from one run is part of the requirement.

  Emulator: the scenario stands the stage up (AWS_PROXY Lambda integration behind GET /ping, account cloudwatchRoleArn set, stage deployed with accessLogSettings) and then re-issues the request while re-reading the destination log group. No record ever appears — the replay never gets past the request/read loop and the suite reports `Error: Check timed out after 60.0s`. Because the record is absent as a whole, not merely one unresolved variable, no status/path/stage/resourcePath/requestId assertion is ever reached and the check produces no verdict.

  Why: a client that configures stage access logging gets a permanently empty log group. Per-request status, path, stage and request id are unavailable, request-id correlation between the gateway and the integration is impossible, and any alarm, metric filter, subscription filter or audit pipeline built on API Gateway access logs never fires. Code that waits for the first access-log record (as this check does) blocks instead of failing fast.

  Fix: the execute-api data plane's access-log emitter for a stage carrying accessLogSettings — it must render the configured format with the per-request $context values and PutLogEvents one record per served request into destinationArn; the stage-settings path that stores accessLogSettings (UpdateStage/CreateStage) must make those settings reach that emitter.

  Related open issue, which does NOT carry this check's replay key and so does not cover it: 20261107T222601Z-apigateway-lambda-logs-a-stage-with-accesslogsettings-and.md (and, for neighbouring surfaces, 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md, 20261108T120220Z-apigateway-dynamodb-lambda-logs-a-rest-stage-with.md). A fixer should expect the same access-log emitter to be the common component.

  Traces: apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group

## Evidence
Check apigateway__lambda__logs.apigw-stage-access-logs-delivered-to-log-group (verdict: mismatch).
  Setup: Lambda returning {"statusCode":200,"headers":{"Content-Type":"application/json"},"body":"{\"ok\":true}"} wired as an AWS_PROXY integration behind GET /ping; account cloudwatchRoleArn configured; stage 'test' deployed with accessLogSettings{destinationArn=<log-group arn>, format={"requestId":"$context.requestId","httpMethod":"$context.httpMethod","path":"$context.path","status":"$context.status","stage":"$context.stage","marker":"$context.resourcePath"}}.
  Expected (AWS): GET /test/ping answers 200 and the destination log group receives one record parseable as that JSON, with httpMethod = the verb sent ('GET'), status = the status returned ('200'), stage = the stage name ('test'), marker ($context.resourcePath) = '/ping', path ($context.path) = the stage-qualified '/test/ping', and a non-empty requestId.
  Actual (emulator): no record containing the stage-qualified path needle ever appears in the log group; the request/read loop never satisfies its condition and the suite reports `Error: Check timed out after 60.0s`. No $context assertion is reached, i.e. zero access-log records are delivered rather than records with wrong values.

## Steps to Reproduce
1. Create a Lambda whose handler returns {"statusCode":200,"headers":{"Content-Type":"application/json"},"body":"{\"ok\":true}"}, and create a CloudWatch Logs log group.
2. UpdateAccount so cloudwatchRoleArn is set (otherwise UpdateStage refuses the accessLogSettings patch).
3. CreateRestApi; CreateResource pathPart='ping'; PutMethod GET authorizationType=NONE; PutIntegration type=AWS_PROXY integrationHttpMethod=POST uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations; lambda AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
4. CreateDeployment/CreateStage for stage 'test' and UpdateStage with accessLogSettings{destinationArn=<log-group arn>, format=<the $context JSON above>}.
5. GET https://<api-id>.execute-api.<region>.<host>/test/ping — expect 200.
6. FilterLogEvents on the log group from ~60 s before the request, re-issuing the GET between reads. Expected: one access-log record per served request with the $context values resolved. Observed: the log group stays empty and the scenario never reaches a verdict.
