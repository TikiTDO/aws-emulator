Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected|_wait_stable#28|Timeout|time.sleep(0.5)
Issue-Commit: ffff878d1ab3490e8981e85eb3beee42e33325a0

# apigatewayv2: an HTTP API stage whose AWS_PROXY IntegrationUri resolves through ${stageVariables.*} still never produces any HTTP response — the ungranted-target 500 and the per-stage stage-variable resolution reach no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `ffff878d1ab3490e8981e85eb3beee42e33325a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi

## Description
One net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED in this replay, so it is filed here.

Cause 1 of 1: the HTTP execute-api data plane never answers a request to a stage whose AWS_PROXY integration target is resolved from a stage variable, so neither the documented refusal nor the positive control is reachable.
Failing check ids for this cause: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected

AWS: an HTTP API integration may embed a stage variable in its Lambda target. Smithy: CreateIntegrationRequest$IntegrationUri — "For a Lambda integration, specify the URI of a Lambda function."; CreateStageRequest$StageVariables — a map defining the stage variables for a Stage (names alphanumeric/underscore, values matching [A-Za-z0-9-._~:/?#&=,]+). For an IntegrationUri of the form arn:aws:lambda:<region>:<account>:function:${stageVariables.<name>}, the gateway substitutes the value taken from the INVOKED stage's StageVariables before target lookup, so two stages sharing one integration/route/deployment resolve to two different functions. Recorded behavior only for the data-plane statuses (the model states no data-plane status): when the resolved function carries no lambda:InvokeFunction resource-policy grant for Principal apigateway.amazonaws.com with a matching SourceArn (arn:aws:execute-api:<region>:<account>:<api-id>/*/*), the gateway fails closed with HTTP 500 and a body that is exactly {"message":"Internal Server Error"} — that whole sentence is fixed text, it has no variable part — and the resolved function is never invoked (no side effect reaches the backend). A sibling stage on the SAME deployment whose stage variable names a permitted function keeps answering 200 with that function's own response body, and that invocation's event carries stageVariables equal to that stage's own variable map and requestContext.stage equal to that stage's name. Granting the resource policy alone (no API, route, integration or deployment change) makes the previously refused stage serve its OWN target with 200. The 500 refusal is repeatable, not a one-off.

Emulator: no stage answers at all. Both the ungranted stage's invoke URL and the permitted twin's invoke URL produce no HTTP response — the in-flight GET on the invoke URL never returns and the check is killed at the 60 s harness cap (verdict: mismatch, "Error: Check timed out after 60.0s"). There is no status line, no error code and no body, so the 500 / {"message":"Internal Server Error"} refusal contract, the never-invoked guarantee, the per-stage stageVariables and requestContext.stage projection, and the repair-by-grant transition are all unobservable.

Why: a client that points an integration at ${stageVariables.*} gets a hung socket instead of a status. It can rely neither on the fail-closed 500 when the resolved target denies invoke, nor on being served when it permits it; because the request never terminates, client-side timeouts and retries fire in place of the gateway's own answer, and the per-stage fan-out of one deployment to several Lambda targets — the entire purpose of stage variables — is unusable.

Fix: the apigatewayv2 HTTP execute-api data-plane request path — the stage/integration resolution step, which must substitute ${stageVariables.<name>} from the invoked stage's StageVariables before target lookup, and the AWS_PROXY invoke step's lambda:InvokeFunction resource-policy check, which must always emit a response (500 with body exactly {"message":"Internal Server Error"} when the resolved target grants no invoke) instead of leaving the connection open.

Related but not covering: open issue 20261109T024519Z-apigatewayv2-an-http-api-stage-whose-aws-proxy.md describes the same scenario but carries a different replay key (…|_wait_stable#27|Timeout|time.sleep(0.5)) and this replay did not mark the check as owned by it; open issue 20261107T125951Z-apigatewayv2-7-net-new-root-causes-where-the.md names this check under its Cause 2 without carrying its replay key; open issue 20261112T084959Z-apigatewayv2-3-net-new-causes-in-the-http.md touches stage-variable integrations in the CORS layer. None of them owns this check.

Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected.

## Evidence
Cause 1 — replay verdict:
  [mismatch] apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected — Error: Check timed out after 60.0s

Expected (AWS):
  GET <invoke-url of the stage whose ${stageVariables.fn} names a Lambda with NO apigateway invoke grant> -> HTTP 500, body exactly {"message":"Internal Server Error"} (fixed text, no variable part), and that Lambda is never invoked.
  GET <invoke-url of the sibling stage on the same deployment whose ${stageVariables.fn} names a permitted Lambda> -> HTTP 200 with that function's own body; the recorded invocation's stageVariables equals that stage's variable map and requestContext.stage equals that stage's name.
  After lambda:AddPermission grants apigateway.amazonaws.com lambda:InvokeFunction on the previously ungranted function (SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/*/*), the same stage -> HTTP 200 served by its OWN target.

Actual (emulator):
  No HTTP response on either stage's invoke URL. The request never completes; the check is terminated at the 60 s cap with no status code, no error code and no body — not even the positive control (the permitted stage returning 200 with its marker) is reached. The refusal's status, error code and message are absent rather than 500 / {"message":"Internal Server Error"}.

## Steps to Reproduce
Replay the behavioral suite and re-run apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected.

Manually:
1. Create two Lambda functions (fn_ok, fn_no) sharing an execution role that may write to a side channel (e.g. an SQS queue).
2. CreateApi(ProtocolType="HTTP"); CreateIntegration(IntegrationType="AWS_PROXY", IntegrationMethod="POST", PayloadFormatVersion="2.0", IntegrationUri="arn:aws:lambda:<region>:<account>:function:${stageVariables.fn}"); CreateRoute(RouteKey="GET /who", Target="integrations/<integration-id>"); CreateDeployment.
3. CreateStage(StageName="granted", DeploymentId=<id>, StageVariables={"fn": fn_ok}) and CreateStage(StageName="ungranted", DeploymentId=<id>, StageVariables={"fn": fn_no}).
4. lambda:AddPermission on fn_ok ONLY: Principal=apigateway.amazonaws.com, Action=lambda:InvokeFunction, SourceArn=arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
5. GET <api-endpoint>/granted/who — expect 200 with fn_ok's body.
6. GET <api-endpoint>/ungranted/who — expect 500 with body exactly {"message":"Internal Server Error"}, and confirm fn_no was never invoked (side channel, using the permitted stage's later request as the ordering barrier).
7. AddPermission the same grant on fn_no and GET the "ungranted" stage again — expect 200 served by fn_no, with the invocation's stageVariables == {"fn": fn_no} and requestContext.stage == "ungranted".

Observed: steps 5-7 never return an HTTP response at all; the request hangs until the harness 60 s cap.
