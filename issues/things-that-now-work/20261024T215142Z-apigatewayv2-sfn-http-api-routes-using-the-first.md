Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# apigatewayv2__sfn: HTTP API routes using the first-class StepFunctions-StartExecution / StartSyncExecution integration subtypes never answer — all three checks hang to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StepFunctions-StartExecution

## Description
Cause 1 of 1: a deployed HTTP API ($default stage, AutoDeploy) whose route targets an AWS_PROXY integration with IntegrationSubtype 'StepFunctions-StartExecution' or 'StepFunctions-StartSyncExecution' never serves the POST, so nothing is ever started in Step Functions. Related open issues describe the same data plane for other check groups but do not carry these checks' replay keys and do not mention the Step Functions subtypes: 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md, 20261024T120951Z-apigatewayv2-cloudwatch-no-http-api-request-metrics-can.md, and (for REST APIs, a different service) 20261023T210711Z-apigateway-sfn-all-three-api-gateway-to-step.md.
AWS: recorded behavior plus the AWS 'Integration subtype reference' for HTTP APIs — an AWS_PROXY integration with IntegrationSubtype StepFunctions-StartExecution (PayloadFormatVersion 1.0, CredentialsArn, RequestParameters StateMachineArn required and Name/Input/Region optional, mappable from $request.* ) makes the stage URL return the StartExecution response body: HTTP 200 with executionArn and startDate, the execution named by the mapped Name and its input byte-for-byte the mapped $request.body. With StepFunctions-StartSyncExecution the response is the StartSyncExecution result (status SUCCEEDED/FAILED plus output) for an EXPRESS state machine. When the integration's CredentialsArn role lacks states:StartExecution, the gateway must surface Step Functions' own refusal and start nothing (DescribeExecution on the named ARN is ExecutionDoesNotExist).
Emulator: the control plane accepts CreateApi/CreateIntegration (GetIntegration reads the subtype back)/CreateRoute/CreateStage, but the POST to the $default stage invoke URL never reaches an accepting response: each check exhausts its 60 s budget in the poll that waits for the first non-retryable HTTP status from the stage URL, before any assertion on the response body runs. Hypothesis (not stated by the recording): the execute-api data plane either does not route the stage path at all or does not implement AWS-service integration subtypes, so the request never completes.
Why: every HTTP API → Step Functions front door is dead on the emulator. A client that fronts a workflow with an HTTP API (the standard serverless 'start a workflow from HTTP' pattern) gets no response at all — no executionArn to hold onto, no synchronous output, and no way to observe that an unauthorized integration role is refused — instead of a 200 StartExecution/StartSyncExecution envelope. Because the call hangs rather than failing fast, callers also burn their own timeouts.
Fix: the apigatewayv2 execute-api data-plane listener/route dispatch for HTTP APIs, and the AWS-service integration-subtype invoker that must sign as CredentialsArn and call Step Functions StartExecution/StartSyncExecution with the mapped request parameters, returning that response as the HTTP body.
Traces: apigatewayv2__sfn.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body, apigatewayv2__sfn.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output, apigatewayv2__sfn.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing.

## Evidence
All three checks: verdict 'mismatch', detail 'Check timed out after 60.0s'.
expected (startexecution subtype): POST <invoke-url>/start with body '{"orderId": "...",  "lines":[...], "note":"caf\\u00e9", "amount":42.5}' and header x-execution-name -> HTTP 200 whose JSON carries executionArn == arn:aws:states:<region>:<account>:execution:<smName>:<mapped name> and startDate; DescribeExecution then reports input equal to the posted bytes, status SUCCEEDED and that payload as output.
actual: the check never gets past _poll_http(..., accept=lambda s: s == 200, description='POST /start reaching the StepFunctions-StartExecution integration') — no accepted response inside the 60 s cap.
expected (startsyncexecution subtype): HTTP 200 carrying the EXPRESS StartSyncExecution result (status SUCCEEDED, output of the priced order).
expected (negative check): the route whose CredentialsArn role lacks states:StartExecution is refused with Step Functions' own error and DescribeExecution on the named ARN is ExecutionDoesNotExist, while the sibling permitted route starts executions before and after.
actual for both: same 60 s hang at the stage URL.

## Steps to Reproduce
1) CreateStateMachine (STANDARD, a single Pass state) and an apigateway.amazonaws.com-assumable role allowing states:StartExecution on it.
2) apigatewayv2 CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='StepFunctions-StartExecution', PayloadFormatVersion='1.0', CredentialsArn=<role>, RequestParameters={'StateMachineArn':<smArn>,'Name':'$request.header.x-execution-name','Input':'$request.body'}); CreateRoute(RouteKey='POST /start', Target='integrations/<id>'); CreateStage(StageName='$default', AutoDeploy=True).
3) POST the invoke URL https://<apiId>.execute-api.<region>.<host>/start with a JSON body and header x-execution-name.
Expected HTTP 200 with executionArn/startDate; observed no response within 60 s. Repeat with IntegrationSubtype='StepFunctions-StartSyncExecution' against an EXPRESS state machine, and with a CredentialsArn role lacking states:StartExecution.
