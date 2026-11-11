Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|run#31|AssertionError|assert record.get("path") == f"/items/{item}", (
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# HTTP API access log $context.path expands to the emulator's internal front-door path '/_aws/execute-api/<apiId>/<path>' instead of the request path

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
One net-new root cause.

Cause 1 of 1: the `$context.path` access-log variable is expanded from the emulator's own front-door request target rather than from the API-relative request path, so every delivered access log record reports an internal routing path that no AWS client can ever observe.

AWS: `AccessLogSettings.Format` is modelled as "A single line format of the access logs of data, as specified by selected $context variables. The format must include at least $context.requestId." (Smithy: apigatewayv2 `AccessLogSettings$Format`). For an HTTP API, AWS expands `$context.path` to the path of the request as the client addressed the API — the route-matched request path, not the route template and not any infrastructure prefix. For a `$default` stage, whose invoke URL carries no stage segment, a `GET /items/{item}` route hit with `/items/one` produces `$context.path` = `/items/one`. The variable parts here are entirely request-derived: the value is the request's own path, whatever it is; `$context.routeKey` (which the emulator does get right, `GET /items/{item}`) is the place the template belongs. Recorded behavior confirms it: in the recording the other fields of the same record — `requestId`, `routeKey`, `status` — matched, and only `path` diverged.

Emulator: the record delivered to the configured CloudWatch Logs group carries `path` = the emulator's internal front-door target, of the form `/_aws/execute-api/<apiId>/<request-path>` (the replay shows `'/_aws/execute-api/mp67t4dbd4/items/one'` where `/items/one` was expected; `mp67t4dbd4` is that run's generated api id and is not part of the requirement — the requirement is that no `/_aws/execute-api/<apiId>` prefix appears at all). The access log is otherwise delivered and otherwise correct, so this is purely the `$context.path` expansion.

Why: access logs are the join key between a gateway's view of a request and the client's view of it. A client that filters or groups access log records by path (per-endpoint latency/error dashboards, metric filters keyed on a path prefix, log-based attribution of a request to an endpoint) matches nothing on AWS-shaped patterns, because every record is prefixed with a path segment that does not exist in AWS. It also leaks the emulator's internal addressing into data a customer stores and reads back, so records captured against the emulator are not comparable with records captured against AWS. The same defect would corrupt any other front-door-derived `$context` variable expanded from the same value.

Fix: in the apigatewayv2 execute-api data plane, the component that builds the access-log record and expands `$context` variables must expand `$context.path` from the API-relative request path — the path after the front-door routing prefix (`/_aws/execute-api/<apiId>`) and after the stage segment have been stripped — i.e. the same normalisation that route matching already applies when it resolves `GET /items/{item}`. The prefix stripping belongs where the request is admitted to the API, so that every downstream consumer (access-log `$context` expansion and the proxy payload's path fields) sees one normalised request path rather than the socket-level target.

Traces: behavioral check `apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id` (mismatch; AssertionError "the record for '18e6e61ccf1c3e11' must report the REQUEST path '/items/one' rather than the route template, got '/_aws/execute-api/mp67t4dbd4/items/one'").

Note: open issue 20261107T070300Z-apigatewayv2-lambda-logs-http-api-access-log-records-are-never-delivered.md covers access-log records NOT being delivered at all; here they ARE delivered (the check got past delivery, requestId, status and routeKey), so that issue does not cover this field-value divergence. The replay report did not mark this check as owned by any open issue.

## Evidence
Check: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id (verdict: mismatch).

Setup (from the replayed check): CreateApi(ProtocolType=HTTP) -> CreateIntegration(AWS_PROXY, PayloadFormatVersion=2.0) -> CreateRoute(RouteKey="GET /items/{item}") -> CreateStage(StageName="$default", AutoDeploy=true, AccessLogSettings={DestinationArn=<log group arn>, Format=JSON with marker/$context.requestId/$context.routeKey/$context.path/$context.status}).

Request: GET <stage url>/items/one -> 200 from the integration, which echoes requestContext.requestId back to the client.

Expected delivered access-log record (fields asserted):
  status   = "200"            [OK]
  routeKey = "GET /items/{item}" [OK]
  path     = "/items/one"      <-- the request path

Actual delivered access-log record:
  path     = "/_aws/execute-api/mp67t4dbd4/items/one"

Failure text: AssertionError: the record for '18e6e61ccf1c3e11' must report the REQUEST path '/items/one' rather than the route template, got '/_aws/execute-api/mp67t4dbd4/items/one'

Fixed text of the requirement: the record's `path` equals the request path. Variable parts: the path itself comes from the request (`/items/<item>`); `mp67t4dbd4` and `18e6e61ccf1c3e11` are a generated api id and a generated request id from this run and are not part of the requirement. The `/_aws/execute-api/<apiId>` prefix is the emulator's internal front-door target and must never appear in `$context.path`.

## Steps to Reproduce
1. CreateApi(Name=<any>, ProtocolType="HTTP") and note ApiId/ApiEndpoint.
2. CreateIntegration(ApiId, IntegrationType="AWS_PROXY", IntegrationUri=<lambda arn>, IntegrationMethod="POST", PayloadFormatVersion="2.0").
3. CreateRoute(ApiId, RouteKey="GET /items/{item}", Target="integrations/<integrationId>").
4. logs:CreateLogGroup(logGroupName="/sim2real/xsvc/accesslogs").
5. CreateStage(ApiId, StageName="$default", AutoDeploy=true, AccessLogSettings={DestinationArn="arn:aws:logs:<region>:<account>:log-group:/sim2real/xsvc/accesslogs", Format='{"requestId":"$context.requestId","routeKey":"$context.routeKey","path":"$context.path","status":"$context.status"}'}).
6. Grant lambda:InvokeFunction to apigateway.amazonaws.com for arn:aws:execute-api:<region>:<account>:<apiId>/*/*.
7. GET <stage invoke url>/items/one and confirm 200.
8. logs:FilterLogEvents on the log group and parse the delivered JSON record for that request.
9. Observe `path` = "/_aws/execute-api/<apiId>/items/one"; AWS reports "/items/one". `routeKey` and `status` are already correct, so only the $context.path expansion diverges.
